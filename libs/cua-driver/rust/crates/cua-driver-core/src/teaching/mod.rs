//! Shared teaching session, normalized events and platform backend boundary.
pub mod evidence;
mod output;
mod timeline;
use crate::teaching;
use crate::{
    perception_client::PerceptionClientHandle,
    protocol::ToolResult,
    tool::{Tool, ToolDef, ToolRegistry},
};
use async_trait::async_trait;
use output::persist_final;
pub use output::SessionOutput;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub use timeline::{inserted_text, push, Action, Source, TextCapture, Timeline};
#[derive(Default)]
pub struct TeachingState {
    state: Mutex<(bool, usize)>,
}
pub struct ActivityGuard {
    state: Arc<TeachingState>,
    teaching: bool,
}
impl TeachingState {
    pub fn start(self: &Arc<Self>) -> Result<ActivityGuard, &'static str> {
        let mut state = self.state.lock().unwrap();
        if state.0 || state.1 != 0 {
            return Err("teaching_busy");
        }
        state.0 = true;
        Ok(ActivityGuard {
            state: self.clone(),
            teaching: true,
        })
    }
    pub fn admit(
        self: &Arc<Self>,
        name: &str,
        read_only: bool,
    ) -> Result<Option<ActivityGuard>, &'static str> {
        if read_only
            || matches!(
                name,
                "start_teaching" | "get_teaching_state" | "stop_teaching" | "end_session"
            )
        {
            return Ok(None);
        }
        let mut state = self.state.lock().unwrap();
        if state.0 {
            return Err("teaching_active");
        }
        state.1 += 1;
        Ok(Some(ActivityGuard {
            state: self.clone(),
            teaching: false,
        }))
    }
}
impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let mut state = self.state.state.lock().unwrap();
        if self.teaching {
            state.0 = false;
        } else {
            state.1 -= 1;
        }
    }
}

/// Semantic key information. Native virtual key codes stay in the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Named(&'static str),
    Modifier,
    Composition,
    Paste,
    Text,
}
impl Key {
    fn name(self) -> Option<String> {
        match self {
            Self::Character(' ') => Some("Space".into()),
            Self::Character(c) => Some(c.to_string()),
            Self::Paste => Some("V".into()),
            Self::Named(s) => Some(s.into()),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Key(Key, u8), // shift/control/alt/meta semantic modifier bits
    Click(i32, i32, &'static str),
    Scroll(i32, i32, i32, bool),
    Focus,
}
#[derive(Clone, Copy, Debug)]
pub struct Raw {
    pub window: u64,
    /// Opaque backend focus token; the core only compares equality.
    pub focus: u64,
    pub pid: u32,
    pub at: u64,
    pub kind: Kind,
}
#[derive(Default)]
pub struct Counters {
    pub dropped: AtomicU64,
}
#[derive(Clone, Default)]
pub struct Context {
    pub safe: bool,
    pub sensitive: bool,
    pub focus: String,
    pub click_target: String,
    pub element: Value,
    pub value: Option<String>,
    pub metadata: Value,
    /// Native proof is interpreted only by the backend that produced it.
    pub proof: Option<Arc<dyn std::any::Any + Send + Sync>>,
}
pub struct Captured {
    pub png: Vec<u8>,
    pub detail: Value,
    pub width: u32,
    pub height: u32,
}
pub trait Listener: Send {
    fn stop(&self);
    fn failure_reason(&self) -> Option<&'static str> {
        None
    }
}
pub trait Backend: Send + Sync + 'static {
    fn check_available(&self) -> Result<(), String>;
    fn start(
        &self,
        tx: SyncSender<Raw>,
        accepting: Arc<AtomicBool>,
        counters: Arc<Counters>,
        excluded: Vec<u32>,
    ) -> Result<Box<dyn Listener>, String>;
    fn current(&self) -> Option<Raw>;
    fn context(&self, event: Raw) -> Context;
    fn is_transient(&self, event: Raw) -> bool;
    fn capture(
        &self,
        event: Raw,
        proof: &(dyn std::any::Any + Send + Sync),
        queued: Instant,
        trigger: &'static str,
    ) -> Result<Captured, &'static str>;
    fn double_click_settings(&self) -> (u64, i64);
    /// Optional native diagnostics retain existing payload fields, not key semantics.
    fn key_metadata(&self, key: Key) -> Value;
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct Live {
    pub timeline: Arc<Mutex<Timeline>>,
    pub accepting: Arc<AtomicBool>,
    pub reason: Arc<Mutex<String>>,
    pub done: mpsc::Receiver<()>,
    pub evidence: Arc<evidence::EvidenceLog>,
    pub cancelled: bool,
}
impl Live {
    pub fn stop(&mut self, reason: &str) {
        if self.timeline.lock().unwrap().stopped.is_some() {
            return;
        }
        *self.reason.lock().unwrap() = reason.into();
        self.evidence.close_capture();
        self.accepting.store(false, Ordering::Release);
        if self.done.recv_timeout(Duration::from_secs(5)).is_err() {
            self.evidence.freeze();
            let mut timeline = self.timeline.lock().unwrap();
            timeline.dropped += 1;
            timeline.finish("stop_timeout");
            persist_final(&mut timeline, &self.evidence);
        }
    }
}
impl Drop for Live {
    fn drop(&mut self) {
        self.stop("owner_disconnected");
    }
}
struct Manager {
    backend: Arc<dyn Backend>,
    live: Option<Live>,
    perception: PerceptionClientHandle,
    exclusion: Arc<teaching::TeachingState>,
}

pub fn run(
    backend: Arc<dyn Backend>,
    rx: mpsc::Receiver<Raw>,
    hooks: Box<dyn Listener>,
    timeline: Arc<Mutex<Timeline>>,
    accepting: Arc<AtomicBool>,
    reason: Arc<Mutex<String>>,
    counters: Arc<Counters>,
    excluded: Vec<u32>,
    max_seconds: u64,
    evidence: Arc<evidence::EvidenceLog>,
) {
    let started = Instant::now();
    let mut last_window = 0;
    let mut text = TextCapture::default();
    let mut queued_raw = None;
    let mut last_desktop_check = Instant::now();
    let mut stopping: Option<Instant> = None;
    let mut evidence_due: Option<(Raw, Context, &'static str)> = None;
    let mut last_input_at = now();
    loop {
        if !accepting.load(Ordering::Acquire) && stopping.is_none() {
            if let Some(failure) = hooks.failure_reason() {
                *reason.lock().unwrap() = failure.into();
            }
            hooks.stop();
            stopping = Some(Instant::now());
        }
        if stopping.is_some_and(|since| since.elapsed() >= Duration::from_millis(500)) {
            break;
        }
        if started.elapsed() >= Duration::from_secs(max_seconds) {
            *reason.lock().unwrap() = "time_limit".into();
            break;
        }
        if last_desktop_check.elapsed() >= Duration::from_millis(250) {
            last_desktop_check = Instant::now();
            if backend.check_available().is_err() {
                *reason.lock().unwrap() = "desktop_unavailable".into();
                break;
            }
        }
        let received = queued_raw.take().or_else(|| {
            if stopping.is_some() {
                rx.try_recv().ok()
            } else {
                rx.recv_timeout(Duration::from_millis(60)).ok()
            }
        });
        if stopping.is_some() && received.is_none() {
            break;
        }
        let raw = match received.or_else(|| backend.current()) {
            Some(r) => r,
            None => continue,
        };
        if received.is_some() {
            last_input_at = raw.at;
        }
        // Candidate popups belong to an IME, not to the demonstrated workflow.
        // Keep the business field baseline while the user chooses a candidate.
        let candidate = backend.is_transient(raw);
        if candidate {
            continue;
        }
        if excluded.contains(&raw.pid) {
            text.baseline = None;
            text.pending = None;
            continue;
        }
        let ctx = backend.context(raw);
        // One-event lookahead preserves ordering between an in-flight UIA
        // sample and input that arrived while that sample was being read.
        queued_raw = rx.try_recv().ok();
        let mut t = timeline.lock().unwrap();
        if t.stopped.is_some() {
            break;
        }
        if matches!(raw.kind, Kind::Focus | Kind::Key(..)) && last_window != raw.window {
            if text.pending.take().is_some() {
                t.text_unavailable += 1;
            }
            text.baseline = None;
            if last_window != 0 {
                let app = ctx.metadata["processName"].as_str().map(str::to_owned);
                push(
                    &mut t,
                    raw,
                    &metadata_only(&ctx),
                    "appSwitch",
                    json!({"toApp":app.clone().unwrap_or_else(||format!("pid:{}",raw.pid)),"toProcessName":app}),
                );
            }
            last_window = raw.window;
        }
        if !ctx.safe {
            if received.is_some() {
                evidence.skip(
                    raw,
                    "input",
                    if ctx.sensitive {
                        "password_control"
                    } else {
                        "context_unavailable"
                    },
                );
            }
            evidence_due = None;
            text.baseline = None;
            if text.pending.take().is_some() {
                t.text_unavailable += 1;
            }
            if received.is_some() {
                if ctx.sensitive {
                    t.redacted += 1;
                } else {
                    t.context_unavailable += 1;
                }
                match raw.kind {
                    Kind::Click(x, y, button) => push(
                        &mut t,
                        raw,
                        &metadata_only(&ctx),
                        "click",
                        json!({"x":x,"y":y,"button":button,"count":1}),
                    ),
                    Kind::Scroll(x, y, delta, horizontal) => push(
                        &mut t,
                        raw,
                        &metadata_only(&ctx),
                        "scroll",
                        json!({"x":x,"y":y,"deltaX":if horizontal {delta}else{0},"deltaY":if horizontal {0}else{delta}}),
                    ),
                    _ => push(
                        &mut t,
                        raw,
                        &metadata_only(&ctx),
                        "secureField",
                        json!({"reason":"sensitive_field","redacted":true}),
                    ),
                }
            }
            t.flush_idle(now());
            continue;
        }
        if let Some((input, value)) =
            text.observe(raw, &ctx, queued_raw, now(), &mut t.text_unavailable)
        {
            push(&mut t, input, &ctx, "typeText", json!({"text":value}));
            evidence_due = Some((input, ctx.clone(), "text_committed"));
        }
        if ctx.value.is_none() {
            if let Some(input) = text.pending {
                // Preserve OCR as evidence; never guess a committed string.
                evidence_due = Some((input, ctx.clone(), "text_without_value_pattern"));
            }
        }
        match raw.kind {
            Kind::Key(key, mods) => {
                if key == Key::Modifier {
                    continue;
                }
                if mods & 14 != 0 && key != Key::Composition {
                    if let Some(name) = key.name() {
                        push(
                            &mut t,
                            raw,
                            &ctx,
                            "hotkey",
                            json!({"keyName":name,"modifiers":modifiers(mods)}),
                        );
                    }
                } else if matches!(key, Key::Named(_)) {
                    if let Some(name) = key.name() {
                        push(&mut t, raw, &ctx, "keyPress", {
                            let mut data = backend.key_metadata(key);
                            data["keyName"] = json!(name);
                            data["modifiers"] = json!(modifiers(mods));
                            data
                        });
                    }
                }
            }
            Kind::Click(x, y, button) => push(
                &mut t,
                raw,
                &ctx,
                "click",
                json!({"x":x,"y":y,"button":button,"count":1}),
            ),
            Kind::Scroll(x, y, delta, horizontal) => push(
                &mut t,
                raw,
                &ctx,
                "scroll",
                json!({"x":x,"y":y,"deltaX":if horizontal {delta}else{0},"deltaY":if horizontal {0}else{delta}}),
            ),
            Kind::Focus => {}
        }
        if matches!(raw.kind, Kind::Click(..)) {
            evidence.request(raw, ctx.clone(), "click");
        }
        if let Some((input, proof, trigger)) = evidence_due.as_ref() {
            if now().saturating_sub(last_input_at) >= 700 && matches!(raw.kind, Kind::Focus) {
                if raw.window == input.window
                    && raw.focus == input.focus
                    && ctx.focus == proof.focus
                {
                    evidence.request(*input, proof.clone(), trigger);
                } else {
                    evidence.skip(*input, trigger, "context_changed");
                }
                evidence_due = None;
            }
        }
        t.flush_idle(now());
    }
    hooks.stop();
    if let Some((raw, _, trigger)) = evidence_due {
        evidence.skip(raw, trigger, "stopped_before_capture");
    }
    evidence.finish(Duration::from_secs(2));
    let mut t = timeline.lock().unwrap();
    if t.stopped.is_none() {
        // A bounded stop discards unprocessed raw events explicitly, rather
        // than looking up today's context for pre-stop events after the fact.
        t.dropped += rx.try_iter().count() as u64
            + u64::from(queued_raw.is_some())
            + counters.dropped.load(Ordering::Relaxed);
        if text.pending.is_some() {
            t.text_unavailable += 1;
        }
        t.finish(&reason.lock().unwrap());
    }
    persist_final(&mut t, &evidence);
}

impl Manager {
    fn call(&mut self, name: &str, args: Value) -> Result<ToolResult, String> {
        let session = args["session_id"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 128)
            .ok_or("teaching_session_id_required")?;
        let owner = args["_session_id"]
            .as_str()
            .ok_or("teaching_owner_required")?;
        let transport = args["_transport_session_id"]
            .as_str()
            .ok_or("teaching_owner_required")?;
        let after = args
            .get("after_sequence")
            .map(|v| v.as_u64().ok_or("teaching_invalid_cursor"))
            .transpose()?
            .unwrap_or(0) as usize;
        let limit = args
            .get("timeline_limit")
            .map(|v| {
                v.as_u64()
                    .filter(|n| *n > 0 && *n <= 1000)
                    .ok_or("teaching_invalid_limit")
            })
            .transpose()?
            .unwrap_or(100) as usize;
        let evidence_after = args
            .get("after_evidence_sequence")
            .map(|v| v.as_u64().ok_or("teaching_invalid_evidence_cursor"))
            .transpose()?
            .unwrap_or(0) as usize;
        let evidence_limit = args
            .get("evidence_limit")
            .map(|v| {
                v.as_u64()
                    .filter(|n| *n > 0 && *n <= 1000)
                    .ok_or("teaching_invalid_evidence_limit")
            })
            .transpose()?
            .unwrap_or(100) as usize;
        if name == "start_teaching" {
            if transport.contains(":cli-") && !transport.ends_with(":cli-explicit") {
                return Err("teaching_requires_named_session".into());
            }
            if self
                .live
                .as_ref()
                .is_some_and(|l| l.timeline.lock().unwrap().stopped.is_none())
            {
                return Err("teaching_already_active".into());
            }
            if self
                .live
                .as_ref()
                .is_some_and(|l| l.timeline.lock().unwrap().session == session)
            {
                return Err("teaching_session_reused".into());
            }
            let seconds = args
                .get("max_seconds")
                .map(|v| {
                    v.as_u64()
                        .filter(|n| *n > 0 && *n <= 600)
                        .ok_or("teaching_invalid_duration")
                })
                .transpose()?
                .unwrap_or(600);
            self.backend.check_available()?;
            let mut excluded = vec![std::process::id()];
            if let Some(pids) = args.get("exclude_pids") {
                for pid in pids
                    .as_array()
                    .filter(|v| v.len() <= 32)
                    .ok_or("teaching_invalid_exclusions")?
                {
                    excluded.push(
                        pid.as_u64()
                            .and_then(|v| u32::try_from(v).ok())
                            .filter(|n| *n > 0)
                            .ok_or("teaching_invalid_exclusions")?,
                    );
                }
            }
            let exclusion = self.exclusion.start()?;
            let accepting = Arc::new(AtomicBool::new(true));
            let counters = Arc::new(Counters::default());
            let (tx, rx) = mpsc::sync_channel(256);
            let output = Arc::new(
                teaching::SessionOutput::create(std::path::Path::new(
                    args["output_dir"]
                        .as_str()
                        .ok_or("teaching_output_required")?,
                ))
                .map_err(|_| "teaching_output_failed")?,
            );
            let mut state = Timeline::new(session.into(), owner.into(), transport.into());
            state.output = Some(output.clone());
            output
                .state(&state.snapshot(0, 0))
                .map_err(|_| "teaching_output_failed")?;
            let hooks =
                match self
                    .backend
                    .start(tx, accepting.clone(), counters.clone(), excluded.clone())
                {
                    Ok(h) => h,
                    Err(e) => {
                        state.finish(&e);
                        let _ = output.state(&state.snapshot(0, 0));
                        return Err(e);
                    }
                };
            (state.double_click_ms, state.double_click_distance) =
                self.backend.double_click_settings();
            let timeline = Arc::new(Mutex::new(state));
            let reason = Arc::new(Mutex::new("requested".into()));
            let evidence = Arc::new(evidence::EvidenceLog::new(
                self.perception.clone(),
                output,
                self.backend.clone(),
            ));
            let (done_tx, done) = mpsc::sync_channel(1);
            let (t, a, r, e) = (
                timeline.clone(),
                accepting.clone(),
                reason.clone(),
                evidence.clone(),
            );
            let backend = self.backend.clone();
            thread::Builder::new()
                .name("cua-teaching-events".into())
                .spawn(move || {
                    run(backend, rx, hooks, t, a, r, counters, excluded, seconds, e);
                    drop(exclusion);
                    let _ = done_tx.send(());
                })
                .map_err(|_| "teaching_thread_failed")?;
            self.live = Some(Live {
                timeline,
                accepting,
                reason,
                done,
                evidence,
                cancelled: false,
            });
        }
        let live = self.live.as_mut().ok_or("teaching_not_found")?;
        live.timeline
            .lock()
            .unwrap()
            .check_owner(session, owner, transport)?;
        if name == "stop_teaching" && !live.cancelled {
            let cancel = args
                .get("cancel")
                .map(|v| v.as_bool().ok_or("teaching_invalid_cancel"))
                .transpose()?
                .unwrap_or(false);
            live.stop(if cancel { "cancelled" } else { "requested" });
            if cancel {
                let t = live.timeline.lock().unwrap();
                if t.stopped.as_deref() == Some("stop_timeout") {
                    return Err("teaching_cleanup_pending".into());
                }
                t.output
                    .as_ref()
                    .unwrap()
                    .cancel()
                    .map_err(|_| "teaching_cancel_cleanup_failed")?;
                live.cancelled = true;
            }
        }
        if live.cancelled {
            let t = live.timeline.lock().unwrap();
            let data = json!({"session_id":t.session,"active":false,"cancelled":true,"cutoff_sequence":t.actions.len(),"output_dir":t.output.as_ref().map(|o|o.root.to_string_lossy())});
            return Ok(ToolResult::text(data.to_string()).with_structured(data));
        }
        let mut timeline = live.timeline.lock().unwrap();
        if timeline.stopped.is_none() {
            if let Some(output) = timeline.output.clone() {
                if output.state(&timeline.snapshot(0, 0)).is_err() {
                    timeline.output_errors += 1;
                }
            }
        }
        let data = timeline.snapshot(after, limit);
        drop(timeline);
        let mut result = ToolResult::text("").with_structured(data);
        live.evidence
            .append_page(&mut result, evidence_after, evidence_limit);
        let data = result.structured_content.as_mut().unwrap();
        data["completion_file"] = if data["complete"] == true {
            json!("complete.json")
        } else {
            Value::Null
        };
        result.content[0] = crate::protocol::Content::text(data.to_string());
        Ok(result)
    }
}

struct TeachingTool {
    def: ToolDef,
    manager: Arc<Mutex<Manager>>,
}
#[async_trait]
impl Tool for TeachingTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let manager = self.manager.clone();
        let name = self.def.name.clone();
        match tokio::task::spawn_blocking(move || manager.lock().unwrap().call(&name, args)).await {
            Ok(Ok(result)) => result,
            Ok(Err(code)) => ToolResult::error(&code)
                .with_structured(json!({"success":false,"error":{"code":code}})),
            Err(_) => ToolResult::error("teaching_worker_failed"),
        }
    }
}
pub fn register_into(
    registry: &mut ToolRegistry,
    perception: PerceptionClientHandle,
    backend: Arc<dyn Backend>,
) {
    let manager = Arc::new(Mutex::new(Manager {
        backend,
        live: None,
        perception,
        exclusion: registry.teaching_state(),
    }));
    let weak = Arc::downgrade(&manager);
    registry.retain_session_end_hook(crate::session::register_scoped_session_end_hook(
        move |owner| {
            if let Some(manager) = weak.upgrade() {
                if let Some(live) = manager.lock().unwrap().live.as_mut() {
                    let owned = {
                        let t = live.timeline.lock().unwrap();
                        t.owner == owner || t.transport == owner
                    };
                    if owned {
                        live.stop("owner_disconnected");
                    }
                }
            }
        },
    ));
    for contract in cua_driver_contract::teaching::contracts() {
        registry.register(Box::new(TeachingTool {
            manager: manager.clone(),
            def: ToolDef::from_contract(&contract),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    pub(super) struct UnavailableBackend;
    #[test]
    fn listener_failure_is_not_reported_as_requested_stop() {
        struct Failed;
        impl Listener for Failed {
            fn stop(&self) {}
            fn failure_reason(&self) -> Option<&'static str> {
                Some("native_listener_stopped")
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let output = Arc::new(SessionOutput::create(directory.path()).unwrap());
        let timeline = Arc::new(Mutex::new(Timeline::new(
            "test".into(),
            "owner".into(),
            "transport".into(),
        )));
        let backend: Arc<dyn Backend> = Arc::new(UnavailableBackend);
        let evidence = Arc::new(evidence::EvidenceLog::new(
            PerceptionClientHandle::fixed(crate::perception_client::PerceptionClient::unavailable()),
            output,
            backend.clone(),
        ));
        let (_tx, rx) = mpsc::sync_channel(1);
        run(
            backend,
            rx,
            Box::new(Failed),
            timeline.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new("requested".into())),
            Arc::new(Counters::default()),
            vec![],
            1,
            evidence,
        );
        let state = timeline.lock().unwrap().snapshot(0, 100);
        assert_eq!(state["termination_reason"], "native_listener_stopped");
        assert_eq!(state["complete"], false);
    }
    impl Backend for UnavailableBackend {
        fn check_available(&self) -> Result<(), String> {
            Err("teaching_desktop_unavailable".into())
        }
        fn start(
            &self,
            _: SyncSender<Raw>,
            _: Arc<AtomicBool>,
            _: Arc<Counters>,
            _: Vec<u32>,
        ) -> Result<Box<dyn Listener>, String> {
            Err("teaching_desktop_unavailable".into())
        }
        fn current(&self) -> Option<Raw> {
            None
        }
        fn context(&self, _: Raw) -> Context {
            Context::default()
        }
        fn is_transient(&self, _: Raw) -> bool {
            false
        }
        fn capture(
            &self,
            _: Raw,
            _: &(dyn std::any::Any + Send + Sync),
            _: Instant,
            _: &'static str,
        ) -> Result<Captured, &'static str> {
            Err("desktop_unavailable")
        }
        fn double_click_settings(&self) -> (u64, i64) {
            (500, 4)
        }
        fn key_metadata(&self, _: Key) -> Value {
            json!({})
        }
    }
    #[test]
    fn teaching_and_automation_are_runtime_local_and_release_on_drop() {
        let runtime = Arc::new(TeachingState::default());
        let other = Arc::new(TeachingState::default());
        let action = runtime.admit("click", false).unwrap();
        assert!(matches!(runtime.start(), Err("teaching_busy")));
        drop(action);
        let session = runtime.start().unwrap();
        assert!(matches!(
            runtime.admit("click", false),
            Err("teaching_active")
        ));
        assert!(runtime.admit("get_teaching_state", true).is_ok());
        assert!(other.admit("click", false).is_ok());
        assert!(other.start().is_ok());
        drop(session);
        assert!(runtime.admit("click", false).is_ok());
    }
    #[test]
    fn session_output_is_fresh_and_cancel_only_removes_its_files() {
        let parent = tempfile::tempdir().unwrap();
        let first = SessionOutput::create(parent.path()).unwrap();
        let second = SessionOutput::create(parent.path()).unwrap();
        first
            .write_json("actions/000001.json", &json!({"sequence":1}))
            .unwrap();
        second
            .write_json("actions/000001.json", &json!({"sequence":2}))
            .unwrap();
        fs::write(first.root.join("caller-note.txt"), "keep").unwrap();
        first.cancel().unwrap();
        assert!(!first.root.join("actions/000001.json").exists());
        assert_eq!(
            fs::read_to_string(first.root.join("caller-note.txt")).unwrap(),
            "keep"
        );
        assert!(second.root.join("actions/000001.json").exists());
    }
    fn timeline() -> Timeline {
        Timeline::new("test".into(), "owner".into(), "transport".into())
    }
    fn action(window: u64, at: u64, kind: &'static str, payload: Value) -> Action {
        Action {
            source: Source {
                window,
                pid: 1,
                focus: "edit".into(),
            },
            at,
            kind,
            payload,
        }
    }
    #[test]
    fn incremental_reads_are_immutable_and_stop_is_a_barrier() {
        let mut t = timeline();
        t.push(action(1, 1000, "typeText", json!({"text":"中"})));
        assert_eq!(t.snapshot(0, 100)["action_count"], 0);
        t.push(action(1, 1100, "typeText", json!({"text":"文"})));
        t.push(action(
            2,
            1200,
            "click",
            json!({"x":1,"y":2,"button":"left","count":1}),
        ));
        let page = t.snapshot(0, 1);
        assert_eq!(page["actions"][0]["payload"]["text"], "中文");
        t.finish("requested");
        let final_state = t.snapshot(0, 100);
        t.push(action(1, 1300, "typeText", json!({"text":"late"})));
        t.finish("owner_disconnected");
        assert_eq!(final_state, t.snapshot(0, 100));
        assert_eq!(t.snapshot(1, 1)["actions"][0]["sequence"], 2);
    }
    #[test]
    fn coalescing_respects_focus_window_time_and_button() {
        let mut t = timeline();
        for at in [1000, 1100] {
            t.push(action(
                1,
                at,
                "click",
                json!({"x":10,"y":20,"button":"left","count":1}),
            ));
        }
        t.push(action(
            1,
            1200,
            "click",
            json!({"x":10,"y":20,"button":"right","count":1}),
        ));
        t.finish("requested");
        assert_eq!(t.actions.len(), 2);
        assert_eq!(t.actions[0]["payload"]["count"], 2);
    }
    #[test]
    fn owner_and_unicode_contract() {
        let t = timeline();
        assert_eq!(
            t.check_owner("other", "owner", "transport"),
            Err("teaching_session_mismatch")
        );
        assert_eq!(
            t.check_owner("test", "owner", "forged"),
            Err("teaching_owner_mismatch")
        );
        assert_eq!(inserted_text("你好世界", "你好，世界"), Some("，".into()));
        assert_eq!(inserted_text("old", "new text"), None);
        assert_eq!(inserted_text("密码", ""), None);
    }

    #[test]
    fn scroll_focus_and_idle_commit_boundaries() {
        let mut t = timeline();
        for at in [1000, 1100] {
            t.push(action(1, at, "scroll", json!({"deltaX":0,"deltaY":120})));
        }
        let mut changed = action(1, 1200, "scroll", json!({"deltaX":120,"deltaY":0}));
        changed.source.focus = "other".into();
        t.push(changed);
        assert_eq!(t.actions.len(), 1);
        assert_eq!(t.actions[0]["payload"]["deltaY"], 240);
        t.flush_idle(2000);
        assert_eq!(t.actions.len(), 2);
        let previous = t.snapshot(0, 1);
        t.finish("requested");
        assert_eq!(previous["actions"], t.snapshot(0, 1)["actions"]);
        assert_eq!(t.snapshot(0, 1)["has_more"], true);
    }

    #[test]
    fn loss_and_abnormal_termination_never_claim_complete() {
        for reason in [
            "time_limit",
            "owner_disconnected",
            "desktop_unavailable",
            "stop_timeout",
        ] {
            let mut t = timeline();
            t.finish(reason);
            assert_eq!(t.snapshot(0, 100)["complete"], false);
        }
        let mut t = timeline();
        t.dropped = 1;
        t.finish("requested");
        assert_eq!(t.snapshot(0, 100)["complete"], false);
    }

    #[test]
    fn action_schema_and_unicode_insertion_are_strict() {
        let mut t = timeline();
        t.push(action(23, 1234, "typeText", json!({"text":"中文🙂"})));
        t.finish("requested");
        let keys: std::collections::BTreeSet<_> = t.actions[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "eventId",
                "sequence",
                "occurredAtEpochMs",
                "sourceWindowId",
                "actionType",
                "payload"
            ]
            .into_iter()
            .collect()
        );
        assert_eq!(inserted_text("前后", "前中文🙂后"), Some("中文🙂".into()));
        assert_eq!(inserted_text("甲乙", "丙乙丁"), None);
    }
}

fn modifiers(mods: u8) -> Vec<&'static str> {
    [(1, "shift"), (2, "ctrl"), (4, "alt"), (8, "meta")]
        .iter()
        .filter_map(|(bit, name)| (mods & bit != 0).then_some(*name))
        .collect()
}

fn metadata_only(ctx: &Context) -> Context {
    let mut metadata = ctx.metadata.clone();
    if metadata.is_object() {
        metadata["windowTitle"] = Value::Null;
        metadata["element"] = Value::Null;
    }
    Context {
        metadata,
        ..Default::default()
    }
}
