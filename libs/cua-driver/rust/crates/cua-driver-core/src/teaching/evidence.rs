//! Bounded, session-owned evidence. Pixels are cropped before OCR/storage;
//! public capture/element registries are never involved.
use super::*;
use crate::perception_client::PerceptionCancellation;
use std::collections::BTreeMap;

#[derive(Default)]
struct LogState {
    entries: Vec<Value>,
    pending: BTreeMap<u64, Value>,
    captured: BTreeMap<u64, Value>,
    next_id: u64,
    frozen: bool,
    omitted: u64,
    output_errors: u64,
    last: Option<(u64, u64, &'static str)>,
}
pub struct EvidenceLog {
    backend: Arc<dyn Backend>,
    state: Mutex<LogState>,
    perception: PerceptionClientHandle,
    output: Arc<SessionOutput>,
    cancel: PerceptionCancellation,
    capture_closed: AtomicBool,
}
struct Request {
    log: std::sync::Weak<EvidenceLog>,
    id: u64,
    raw: Raw,
    proof: Arc<dyn std::any::Any + Send + Sync>,
    trigger: &'static str,
    queued: Instant,
}
struct OcrRequest {
    log: std::sync::Weak<EvidenceLog>,
    id: u64,
    png: Vec<u8>,
    detail: Value,
    width: u32,
    height: u32,
}
impl EvidenceLog {
    pub fn new(
        perception: PerceptionClientHandle,
        output: Arc<SessionOutput>,
        backend: Arc<dyn Backend>,
    ) -> Self {
        Self {
            backend,
            state: Mutex::new(LogState::default()),
            perception,
            output,
            cancel: PerceptionCancellation::default(),
            capture_closed: AtomicBool::new(false),
        }
    }
    fn reserve(&self, raw: Raw, trigger: &'static str) -> Option<u64> {
        let mut s = self.state.lock().unwrap();
        if s.frozen || s.last == Some((raw.at, raw.window, trigger)) {
            return None;
        }
        s.last = Some((raw.at, raw.window, trigger));
        if s.entries.len() + s.pending.len() >= 10000 {
            s.omitted += 1;
            return None;
        }
        s.next_id += 1;
        let id = s.next_id;
        s.pending.insert(id,json!({"evidence_id":id,"trigger":trigger,"source_window":{"id":raw.window,"pid":raw.pid},"source_event_at_epoch_ms":raw.at,"relation":"post_event_observation"}));
        Some(id)
    }
    pub fn skip(&self, raw: Raw, trigger: &'static str, reason: &str) {
        if let Some(id) = self.reserve(raw, trigger) {
            self.complete(id, json!({"status":"skipped","reason":reason}));
        }
    }
    pub fn request(self: &Arc<Self>, raw: Raw, proof: Context, trigger: &'static str) {
        let Some(id) = self.reserve(raw, trigger) else {
            return;
        };
        if self.capture_closed.load(Ordering::Acquire) {
            self.complete(
                id,
                json!({"status":"skipped","reason":"stopped_before_capture"}),
            );
            return;
        }
        let Some(native_proof) = proof.proof.filter(|_| proof.safe && !proof.sensitive) else {
            self.complete(
                id,
                json!({"status":"skipped","reason":"privacy_or_context_changed"}),
            );
            return;
        };
        if worker()
            .try_send(Request {
                log: Arc::downgrade(self),
                id,
                raw,
                proof: native_proof,
                trigger,
                queued: Instant::now(),
            })
            .is_err()
        {
            self.complete(
                id,
                json!({"status":"skipped","reason":"evidence_worker_busy"}),
            );
        }
    }
    fn append(&self, s: &mut LogState, mut metadata: Value) {
        let sequence = s.entries.len() + 1;
        let file = format!("evidence/{sequence:06}.json");
        metadata["sequence"] = json!(sequence);
        metadata["file"] = json!(file);
        if self.output.write_json(&file, &metadata).is_err() {
            s.output_errors += 1;
            metadata["storage_error"] = json!("evidence_write_failed");
        }
        s.entries.push(metadata);
    }
    fn complete(&self, id: u64, detail: Value) {
        let mut s = self.state.lock().unwrap();
        if s.frozen {
            return;
        }
        let Some(mut metadata) = s.pending.remove(&id) else {
            return;
        };
        if let Some(captured) = s.captured.remove(&id) {
            metadata
                .as_object_mut()
                .unwrap()
                .extend(captured.as_object().unwrap().clone());
        }
        metadata
            .as_object_mut()
            .unwrap()
            .extend(detail.as_object().unwrap().clone());
        self.append(&mut s, metadata);
    }
    fn stage_image(&self, id: u64, png: &[u8], detail: &Value) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.frozen || !s.pending.contains_key(&id) {
            return false;
        }
        let relative = format!("images/{id:06}.png");
        if self.output.write_new(&relative, png).is_err() {
            s.output_errors += 1;
            drop(s);
            self.complete(
                id,
                json!({"status":"skipped","reason":"image_write_failed"}),
            );
            return false;
        }
        let mut detail = detail.clone();
        detail["image"] = json!(relative);
        detail["byte_count"] = json!(png.len());
        s.captured.insert(id, detail);
        true
    }
    pub fn close_capture(&self) {
        self.capture_closed.store(true, Ordering::Release);
    }
    pub fn finish(&self, budget: Duration) {
        self.close_capture();
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline && !self.state.lock().unwrap().pending.is_empty() {
            thread::sleep(Duration::from_millis(20));
        }
        self.freeze();
    }
    pub fn freeze(&self) {
        self.close_capture();
        self.cancel.cancel();
        let mut s = self.state.lock().unwrap();
        if s.frozen {
            return;
        }
        for (id, mut metadata) in std::mem::take(&mut s.pending) {
            if let Some(captured) = s.captured.remove(&id) {
                metadata
                    .as_object_mut()
                    .unwrap()
                    .extend(captured.as_object().unwrap().clone());
                metadata["ocr"] = json!({"status":"unavailable","reason":"stop_deadline"});
            } else {
                metadata["status"] = json!("skipped");
                metadata["reason"] = json!("stop_deadline");
            }
            self.append(&mut s, metadata);
        }
        s.frozen = true;
    }
    pub fn snapshot(&self, after: usize, limit: usize) -> Value {
        let s = self.state.lock().unwrap();
        let entries = s.entries.iter().skip(after).take(limit).collect::<Vec<_>>();
        let next = after + entries.len();
        json!({"entries":entries,"next_sequence":next,"has_more":next<s.entries.len(),"pending":s.pending.len(),"frozen":s.frozen,"cutoff_sequence":s.frozen.then_some(s.entries.len()),"omitted":s.omitted,"output_errors":s.output_errors,"complete":s.frozen && s.omitted==0 && s.output_errors==0 && s.entries.iter().all(|e| (e["status"]=="captured" && matches!(e["ocr"]["status"].as_str(),Some("available"|"not_needed"))) || e["reason"]=="password_control")})
    }
    pub fn append_page(&self, result: &mut ToolResult, after: usize, limit: usize) {
        let data = result.structured_content.as_mut().unwrap();
        data["evidence"] = self.snapshot(after, limit);
        result.content[0] = crate::protocol::Content::text(data.to_string());
    }
}

fn worker() -> &'static SyncSender<Request> {
    static WORKER: OnceLock<SyncSender<Request>> = OnceLock::new();
    WORKER.get_or_init(|| {
        // One active and one admitted request, never a thread per event. A
        // stalled native provider blocks this lane and later events skip it.
        let (tx, rx) = mpsc::sync_channel::<Request>(1);
        thread::Builder::new()
            .name("cua-teaching-evidence".into())
            .spawn(move || {
                for request in rx {
                    let Some(log) = request.log.upgrade() else {
                        continue;
                    };
                    if log.cancel.is_cancelled() {
                        continue;
                    }
                    if log.capture_closed.load(Ordering::Acquire) {
                        log.complete(
                            request.id,
                            json!({"status":"skipped","reason":"stopped_before_capture"}),
                        );
                        continue;
                    }
                    if request.queued.elapsed() > Duration::from_millis(250) {
                        log.complete(
                            request.id,
                            json!({"status":"skipped","reason":"capture_admission_expired"}),
                        );
                        continue;
                    }
                    let backend = log.backend.clone();
                    drop(log);
                    let capture = backend.capture(
                        request.raw,
                        request.proof.as_ref(),
                        request.queued,
                        request.trigger,
                    );
                    let Some(log) = request.log.upgrade() else {
                        continue;
                    };
                    match capture {
                        Err(reason) => {
                            log.complete(request.id, json!({"status":"skipped","reason":reason}))
                        }
                        Ok(Captured {
                            png,
                            detail,
                            width,
                            height,
                        }) => {
                            if log.cancel.is_cancelled() {
                                continue;
                            }
                            if !log.stage_image(request.id, &png, &detail) {
                                continue;
                            }
                            if request.trigger != "text_without_value_pattern" {
                                let mut detail = detail;
                                detail["ocr"] = json!({"status":"not_needed"});
                                log.complete(request.id, detail);
                                continue;
                            }
                            if let Err(error) = ocr_worker().try_send(OcrRequest {
                                log: request.log,
                                id: request.id,
                                png,
                                detail,
                                width,
                                height,
                            }) {
                                let mut job = match error {
                                    mpsc::TrySendError::Full(job)
                                    | mpsc::TrySendError::Disconnected(job) => job,
                                };
                                job.detail["ocr"] =
                                    json!({"status":"unavailable","reason":"ocr_worker_busy"});
                                log.complete(job.id, job.detail);
                            }
                        }
                    }
                }
            })
            .expect("create evidence worker");
        tx
    })
}
fn ocr_worker() -> &'static SyncSender<OcrRequest> {
    static WORKER: OnceLock<SyncSender<OcrRequest>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<OcrRequest>(1);
        thread::Builder::new().name("cua-teaching-ocr".into()).spawn(move || {
            // Keep upstream idle-release timers alive between queued requests.
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .ok();
            for mut job in rx {
                let Some(log) = job.log.upgrade() else { continue; };
                if log.cancel.is_cancelled() { continue; }
                job.detail["ocr"] = match runtime.as_ref() {
                    Some(runtime) => runtime.block_on(async {
                        let client = log.perception.current().await;
                        let parsed = client.parse(
                            &format!("teaching-{}", job.id), job.width, job.height,
                            &job.png, &log.cancel,
                        ).await;
                        match parsed {
                            Ok(value)=>json!({"status":"available","coordinate_space":"evidence_pixels","regions":text_regions(&value),"parser":value.get("identity").cloned().unwrap_or(Value::Null)}),
                            Err(error)=>json!({"status":"unavailable","reason":format!("{:?}",error.code)}),
                        }
                    }),
                    None=>json!({"status":"unavailable","reason":"runtime_unavailable"}),
                };
                log.complete(job.id, job.detail);
            }
        }).expect("create teaching OCR worker");
        tx
    })
}
fn text_regions(value: &Value) -> Vec<Value> {
    value
        .get("regions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| {
            r.get("text")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        })
        .take(128)
        .map(|r| json!({"text":r["text"],"bounds":r["bounds"],"confidence":r["confidence"]}))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::super::tests::UnavailableBackend;
    use super::*;
    use crate::perception_client::PerceptionClient;
    fn log() -> (tempfile::TempDir, EvidenceLog) {
        let dir = tempfile::tempdir().unwrap();
        let output = Arc::new(SessionOutput::create(dir.path()).unwrap());
        (
            dir,
            EvidenceLog::new(
                PerceptionClientHandle::fixed(PerceptionClient::unavailable()),
                output,
                Arc::new(UnavailableBackend),
            ),
        )
    }
    fn raw() -> Raw {
        Raw {
            window: 5,
            focus: 6,
            pid: 7,
            at: 1000,
            kind: Kind::Focus,
        }
    }
    #[test]
    fn optional_evidence_does_not_change_action_completion_in_files_or_responses() {
        for failure in [
            "capture",
            "ocr",
            "deadline",
            "image_write",
            "action_loss",
            "action_write",
        ] {
            let (_dir, log) = log();
            let id = log.reserve(raw(), "click").unwrap();
            match failure {
                "ocr" => {
                    assert!(log.stage_image(id, &[1], &json!({"status":"captured"})));
                    log.complete(
                        id,
                        json!({"ocr":{"status":"unavailable","reason":"ocr_worker_busy"}}),
                    );
                }
                "image_write" => {
                    std::fs::write(log.output.root.join("images/000001.png"), "caller file")
                        .unwrap();
                    assert!(!log.stage_image(id, &[1], &json!({"status":"captured"})));
                }
                "deadline" => {}
                _ => log.complete(
                    id,
                    json!({"status":"skipped","reason":"target_capture_failed"}),
                ),
            }
            log.freeze();
            let mut timeline = Timeline::new("test".into(), "owner".into(), "transport".into());
            timeline.output = Some(log.output.clone());
            if failure == "action_loss" {
                timeline.dropped = 1;
            }
            if failure == "action_write" {
                std::fs::write(log.output.root.join("actions/000001.json"), "caller file").unwrap();
            }
            timeline.push(Action {
                source: Source {
                    window: 5,
                    pid: 7,
                    focus: "edit".into(),
                },
                at: 1000,
                kind: "typeText",
                payload: json!({"text":"中文"}),
            });
            timeline.finish("requested");
            persist_final(&mut timeline, &log);
            let expected = !matches!(failure, "action_loss" | "action_write");
            let stored: Value = serde_json::from_slice(&fs_read(&log, "session.json")).unwrap();
            assert_eq!(stored["complete"], expected, "{failure}: {stored}");
            assert_eq!(stored["evidence"]["complete"], false);
            assert_eq!(log.output.root.join("complete.json").exists(), expected);
            if expected {
                let marker: Value =
                    serde_json::from_slice(&fs_read(&log, "complete.json")).unwrap();
                assert_eq!(marker["evidence_complete"], false);
                assert_eq!(marker["cutoff_sequence"], 1);
            }
            let mut manager = Manager {
                backend: Arc::new(UnavailableBackend),
                live: Some(Live {
                    timeline: Arc::new(Mutex::new(timeline)),
                    accepting: Arc::new(AtomicBool::new(false)),
                    reason: Arc::new(Mutex::new("requested".into())),
                    done: mpsc::channel().1,
                    evidence: Arc::new(log),
                    cancelled: false,
                }),
                perception: PerceptionClientHandle::fixed(PerceptionClient::unavailable()),
                exclusion: Arc::new(teaching::TeachingState::default()),
            };
            for tool in ["get_teaching_state", "stop_teaching"] {
                let response = manager.call(tool, json!({"session_id":"test", "_session_id":"owner", "_transport_session_id":"transport"})).unwrap().structured_content.unwrap();
                assert_eq!(response["complete"], expected, "{failure}: {response}");
                assert_eq!(response["evidence"]["complete"], false);
                assert_eq!(response["completion_file"].is_string(), expected);
            }
        }
    }
    #[test]
    fn files_are_relative_readable_and_late_results_cannot_change_them() {
        let (_dir, log) = log();
        let id = log.reserve(raw(), "click").unwrap();
        assert!(log.stage_image(id, &[1, 2, 3], &json!({"status":"captured"})));
        log.freeze();
        let first = log.snapshot(0, 10);
        let entry = &first["entries"][0];
        assert_eq!(
            fs_read(&log, entry["image"].as_str().unwrap()),
            vec![1, 2, 3]
        );
        let before = fs_read(&log, entry["file"].as_str().unwrap());
        log.complete(
            id,
            json!({"status":"captured","ocr":{"status":"available"}}),
        );
        assert_eq!(before, fs_read(&log, entry["file"].as_str().unwrap()));
        assert_eq!(first, log.snapshot(0, 10));
        assert_eq!(log.snapshot(1, 10)["entries"], json!([]));
    }
    fn fs_read(log: &EvidenceLog, name: &str) -> Vec<u8> {
        std::fs::read(log.output.root.join(name)).unwrap()
    }
    #[test]
    fn existing_images_are_not_overwritten_and_failure_is_reported() {
        let (_dir, log) = log();
        let id = log.reserve(raw(), "click").unwrap();
        std::fs::write(log.output.root.join("images/000001.png"), "existing").unwrap();
        assert!(!log.stage_image(id, &[9], &json!({"status":"captured"})));
        assert_eq!(fs_read(&log, "images/000001.png"), b"existing");
        assert_eq!(
            log.snapshot(0, 10)["entries"][0]["reason"],
            "image_write_failed"
        );
    }
    #[test]
    fn pending_capture_is_frozen_as_partial_without_late_file_creation() {
        let (_dir, log) = log();
        let id = log.reserve(raw(), "click").unwrap();
        log.freeze();
        assert!(!log.stage_image(id, &[1], &json!({"status":"captured"})));
        assert!(!log.output.root.join("images/000001.png").exists());
        assert_eq!(log.snapshot(0, 10)["entries"][0]["reason"], "stop_deadline");
    }
}
