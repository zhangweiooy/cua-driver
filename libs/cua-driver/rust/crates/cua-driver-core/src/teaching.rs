//! Product teaching data contract and bounded, immutable incremental timeline.
//! Runtime state and session-owned files; no public observation-cache mutations.
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

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

/// Fresh, session-owned output. Oryx reads and retains these files directly.
pub struct SessionOutput {
    pub root: PathBuf,
    files: Mutex<std::collections::BTreeSet<String>>,
}
impl SessionOutput {
    pub fn create(parent: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(parent)?;
        let root = fs::canonicalize(parent)?.join(format!("teaching-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root)?;
        for name in ["actions", "images", "evidence"] {
            fs::create_dir(root.join(name))?;
        }
        Ok(Self {
            root,
            files: Mutex::new(std::collections::BTreeSet::new()),
        })
    }
    pub fn write_new(&self, relative: &str, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join(relative))?;
        self.files.lock().unwrap().insert(relative.into());
        file.write_all(bytes)
    }
    pub fn write_json(&self, relative: &str, data: &Value) -> std::io::Result<()> {
        self.write_new(relative, &serde_json::to_vec_pretty(data)?)
    }
    pub fn state(&self, data: &Value) -> std::io::Result<()> {
        let temp = self.root.join("session.json.tmp");
        self.files
            .lock()
            .unwrap()
            .extend(["session.json.tmp".into(), "session.json".into()]);
        fs::write(&temp, serde_json::to_vec_pretty(data)?)?;
        fs::rename(temp, self.root.join("session.json"))
    }
    pub fn cancel(&self) -> std::io::Result<()> {
        // Only our generated root and recorded file names are deletion targets.
        fn check(path: &Path) -> std::io::Result<()> {
            let metadata = fs::symlink_metadata(path)?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(std::io::Error::other(
                        "output path replaced by reparse point",
                    ));
                }
            }
            if metadata.file_type().is_symlink() {
                return Err(std::io::Error::other("output path replaced by link"));
            }
            Ok(())
        }
        if fs::canonicalize(&self.root)? != self.root {
            return Err(std::io::Error::other("output directory identity changed"));
        }
        check(&self.root)?;
        for name in ["actions", "images", "evidence"] {
            check(&self.root.join(name))?;
        }
        for name in self.files.lock().unwrap().iter() {
            match fs::remove_file(self.root.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        // Unknown files belong to the caller, so a nonempty directory remains.
        for name in ["actions", "images", "evidence"] {
            let _ = fs::remove_dir(self.root.join(name));
        }
        let _ = fs::remove_dir(&self.root);
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub window: u64,
    pub pid: u32,
    pub focus: String,
}

#[derive(Clone, Debug)]
pub struct Action {
    pub source: Source,
    pub at: u64,
    pub kind: &'static str,
    pub payload: Value,
}

/// Pending actions are never published and then modified. Reading a timeline
/// returns only committed entries; stopping flushes the last coalesced action.
pub struct Timeline {
    pub output: Option<Arc<SessionOutput>>,
    pub output_errors: u64,
    pub session: String,
    pub owner: String,
    pub transport: String,
    pub actions: Vec<Value>,
    pending: Option<Action>,
    last_at: u64,
    pub stopped: Option<String>,
    pub dropped: u64,
    pub context_unavailable: u64,
    pub redacted: u64,
    pub text_unavailable: u64,
    pub double_click_ms: u64,
    pub double_click_distance: i64,
}

impl Timeline {
    pub fn new(session: String, owner: String, transport: String) -> Self {
        Self {
            session,
            output: None,
            output_errors: 0,
            owner,
            transport,
            actions: Vec::new(),
            pending: None,
            last_at: 0,
            stopped: None,
            dropped: 0,
            context_unavailable: 0,
            redacted: 0,
            text_unavailable: 0,
            double_click_ms: 500,
            double_click_distance: 4,
        }
    }
    pub fn check_owner(
        &self,
        session: &str,
        owner: &str,
        transport: &str,
    ) -> Result<(), &'static str> {
        if session != self.session {
            return Err("teaching_session_mismatch");
        }
        if owner != self.owner || transport != self.transport {
            return Err("teaching_owner_mismatch");
        }
        Ok(())
    }
    pub fn push(&mut self, next: Action) {
        if self.stopped.is_some() {
            return;
        }
        if self.actions.len() >= 10000 {
            self.dropped += 1;
            self.finish("action_limit");
            return;
        }
        if let Some(previous) = self.pending.as_mut() {
            let gap = next.at.saturating_sub(self.last_at);
            if previous.source == next.source && previous.kind == next.kind {
                let merged = match next.kind {
                    "typeText" if gap <= 700 => {
                        let text = format!(
                            "{}{}",
                            previous.payload["text"].as_str().unwrap_or(""),
                            next.payload["text"].as_str().unwrap_or("")
                        );
                        if text.len() <= 16384 {
                            previous.payload["text"] = json!(text);
                            true
                        } else {
                            false
                        }
                    }
                    "scroll" if gap <= 250 => {
                        for key in ["deltaX", "deltaY"] {
                            previous.payload[key] = json!(
                                previous.payload[key].as_i64().unwrap_or(0)
                                    + next.payload[key].as_i64().unwrap_or(0)
                            );
                        }
                        true
                    }
                    "click"
                        if gap <= self.double_click_ms
                            && previous.payload["count"] == 1
                            && previous.payload["button"] == next.payload["button"] =>
                    {
                        let close = ["x", "y"].iter().all(|key| {
                            (previous.payload[key].as_i64().unwrap_or(0)
                                - next.payload[key].as_i64().unwrap_or(0))
                            .abs()
                                <= self.double_click_distance
                        });
                        if close {
                            previous.payload["count"] = json!(2);
                        }
                        close
                    }
                    _ => false,
                };
                if merged {
                    self.last_at = next.at;
                    return;
                }
            }
        }
        self.flush();
        self.last_at = next.at;
        self.pending = Some(next);
    }
    pub fn flush_idle(&mut self, now: u64) {
        if now.saturating_sub(self.last_at) > 750 {
            self.flush();
        }
    }
    pub fn flush(&mut self) {
        if let Some(action) = self.pending.take() {
            let sequence = self.actions.len() + 1;
            self.actions.push(json!({
                "eventId": format!("{}:{sequence}", self.session), "sequence": sequence,
                "occurredAtEpochMs": action.at, "sourceWindowId": action.source.window.to_string(),
                "actionType": action.kind, "payload": action.payload,
            }));
            if let Some(output) = &self.output {
                if output
                    .write_json(
                        &format!("actions/{sequence:06}.json"),
                        self.actions.last().unwrap(),
                    )
                    .is_err()
                {
                    self.output_errors += 1;
                }
            }
        }
    }
    pub fn finish(&mut self, reason: &str) {
        if self.stopped.is_none() {
            self.flush();
            self.stopped = Some(reason.into());
        }
    }
    pub fn snapshot(&self, after: usize, limit: usize) -> Value {
        let end = after.saturating_add(limit).min(self.actions.len());
        let actions = self.actions.get(after..end).unwrap_or(&[]);
        json!({"teaching_contract":"desktop-action.v1", "stop_barrier":"cutoff-sequence.v1",
            "session_id":self.session, "owner":self.owner,
            "active":self.stopped.is_none(), "actions":actions,
            "next_sequence": if actions.is_empty() { after } else { end },
            "has_more":end < self.actions.len(), "action_count":self.actions.len(),
            "cutoff_sequence":self.stopped.as_ref().map(|_| self.actions.len()),
            "termination_reason":self.stopped,
            "output_dir":self.output.as_ref().map(|o|o.root.to_string_lossy()),"state_file":"session.json","actions_directory":"actions","action_files":actions.iter().map(|a|format!("actions/{:06}.json",a["sequence"].as_u64().unwrap())).collect::<Vec<_>>(),
            "complete":self.stopped.as_deref() == Some("requested") && self.dropped == 0 && self.context_unavailable == 0 && self.text_unavailable == 0 && self.output_errors==0,
            "diagnostics":{"dropped_events":self.dropped,"context_unavailable":self.context_unavailable,
                "redacted_events":self.redacted,"text_unavailable":self.text_unavailable,"output_errors":self.output_errors}})
    }
}

/// Return only a proved insertion, never the entire pre-existing field value.
/// Replacement/deletion and a missing baseline are not guessed as typed text.
pub fn inserted_text(before: &str, after: &str) -> Option<String> {
    let a: Vec<char> = before.chars().collect();
    let b: Vec<char> = after.chars().collect();
    if b.len() <= a.len() {
        return None;
    }
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..].len();
    if a[prefix..] != b[b.len() - suffix..] {
        return None;
    }
    Some(b[prefix..b.len() - suffix].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
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
