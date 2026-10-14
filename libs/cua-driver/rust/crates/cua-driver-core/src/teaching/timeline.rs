use super::*;
use serde_json::{json, Value};
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

pub fn push(t: &mut Timeline, raw: Raw, ctx: &Context, kind: &'static str, extra: Value) {
    let mut payload = if ctx.metadata.is_object() {
        ctx.metadata.clone()
    } else {
        json!({"app":format!("pid:{}", raw.pid),"processName":null,"windowTitle":null,"url":null,"element":if ctx.safe {ctx.element.clone()} else {Value::Null}})
    };
    payload
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    t.push(Action {
        source: Source {
            window: raw.window,
            pid: raw.pid,
            focus: if kind == "click" {
                ctx.click_target.clone()
            } else {
                ctx.focus.clone()
            },
        },
        at: raw.at,
        kind,
        payload,
    });
}

#[derive(Default)]
pub struct TextCapture {
    pub baseline: Option<(u64, String, String)>,
    pub pending: Option<Raw>,
}
fn is_text_input(kind: Kind) -> bool {
    matches!(kind, Kind::Key(key, mods) if matches!(key, Key::Composition | Key::Paste | Key::TextInput)
        || (mods & 14 == 0 && matches!(key, Key::Character(_) | Key::Text))
        || (mods & 2 != 0 && key == Key::Character('V')))
}
impl TextCapture {
    pub fn observe(
        &mut self,
        raw: Raw,
        ctx: &Context,
        queued_input: Option<Raw>,
        sampled_at: u64,
        unavailable: &mut u64,
    ) -> Option<(Raw, String)> {
        if is_text_input(raw.kind) {
            self.pending.get_or_insert(raw);
        }
        let mut committed = None;
        if let Some(value) = ctx.value.as_ref() {
            // UIA may see the paste before its queued key event is processed.
            // Keep the old value until that event can claim the text change.
            let same_focus = self
                .baseline
                .as_ref()
                .is_some_and(|(window, focus, _)| *window == raw.window && *focus == ctx.focus);
            let continues_text = queued_input.is_some_and(|next| {
                next.window == raw.window
                    && next.pid == raw.pid
                    && next.focus == raw.focus
                    && (is_text_input(next.kind)
                        // With no pending text, a modifier may precede a paste
                        // already seen by UIA. Do not advance that baseline.
                        || (self.pending.is_none()
                            && matches!(next.kind, Kind::Key(Key::Modifier, _))))
            });
            if continues_text && same_focus {
                // Defer only a continuation/commit in this field. Clicks,
                // navigation and other sources must first settle known text.
                return None;
            }
            if let Some((_, _, before)) = self.baseline.as_ref().filter(|_| same_focus) {
                if before != value {
                    if let Some(input) = self.pending.take() {
                        if let Some(text) = teaching::inserted_text(before, value) {
                            committed = Some((input, text));
                        } else {
                            *unavailable += 1;
                        }
                    }
                } else if matches!(raw.kind, Kind::Key(Key::Named("Escape"), _)) {
                    // Escape cancelled a composition without changing committed text.
                    self.pending = None;
                }
            } else if self.baseline.is_some() && self.pending.take().is_some() {
                *unavailable += 1;
            }
            self.baseline = Some((raw.window, ctx.focus.clone(), value.clone()));
            // An unchanged readable value is not evidence of lost text. IME
            // composition can legitimately stay open while the user chooses.
        } else {
            self.baseline = None;
            if self
                .pending
                .is_some_and(|input| sampled_at.saturating_sub(input.at) > 1500)
            {
                self.pending = None;
                *unavailable += 1;
            }
        }
        committed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text_sample(kind: Kind, at: u64) -> Raw {
        Raw {
            window: 1,
            focus: 2,
            pid: 3,
            at,
            kind,
        }
    }
    fn text_context(value: Option<&str>) -> Context {
        Context {
            safe: true,
            focus: "field-a".into(),
            value: value.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn platform_text_intent_preserves_modifier_shortcut_semantics() {
        for (key, mods, expected) in [
            (Key::TextInput, 4, true),
            (Key::TextInput, 5, true),
            (Key::Paste, 8, true),
            (Key::Character('V'), 2, true),
            (Key::Character('V'), 4, false),  // Windows Alt+V
            (Key::Character('A'), 12, false), // Command+Option shortcut
            (Key::Character('A'), 6, false),
        ] {
            let mut capture = TextCapture::default();
            let mut missing = 0;
            capture.observe(
                text_sample(Kind::Focus, 0),
                &text_context(Some("")),
                None,
                0,
                &mut missing,
            );
            let committed = capture.observe(
                text_sample(Kind::Key(key, mods), 10),
                &text_context(Some("√")),
                None,
                10,
                &mut missing,
            );
            assert_eq!(
                committed.map(|(_, text)| text),
                expected.then(|| "√".to_owned()),
                "{key:?}, {mods}"
            );
            assert_eq!(missing, 0);
        }
    }

    #[test]
    fn click_coalescing_uses_only_click_target_identity() {
        for same_target in [true, false] {
            let first = Context {
                focus: "text-before".into(),
                click_target: "virtual-click-a".into(),
                ..Default::default()
            };
            let second = Context {
                focus: if same_target {
                    "text-after"
                } else {
                    "text-before"
                }
                .into(),
                click_target: if same_target {
                    "virtual-click-a"
                } else {
                    "virtual-click-b"
                }
                .into(),
                ..Default::default()
            };
            let mut timeline =
                Timeline::new("click-identity".into(), "owner".into(), "transport".into());
            for (at, ctx) in [(100, first), (200, second)] {
                push(
                    &mut timeline,
                    text_sample(Kind::Click(30, 40, "left"), at),
                    &ctx,
                    "click",
                    json!({"x":30,"y":40,"button":"left","count":1}),
                );
            }
            timeline.finish("requested");
            assert_eq!(timeline.actions.len(), if same_target { 1 } else { 2 });
            assert_eq!(
                timeline.actions[0]["payload"]["count"],
                if same_target { 2 } else { 1 }
            );
        }
    }

    #[test]
    fn confirmed_text_is_flushed_before_queued_control_or_window_change() {
        for next in [
            text_sample(Kind::Click(30, 40, "left"), 20),
            Raw {
                focus: 4,
                ..text_sample(Kind::Key(Key::Character('B'), 0), 20)
            },
            Raw {
                window: 9,
                ..text_sample(Kind::Key(Key::Character('B'), 0), 20)
            },
            Raw {
                pid: 9,
                ..text_sample(Kind::Key(Key::Character('B'), 0), 20)
            },
            text_sample(Kind::Key(Key::Named("Tab"), 0), 20),
            text_sample(Kind::Key(Key::Character('A'), 2), 20),
            text_sample(Kind::Scroll(30, 40, 120, false), 20),
        ] {
            let mut capture = TextCapture::default();
            let mut missing = 0;
            capture.observe(
                text_sample(Kind::Focus, 0),
                &text_context(Some("")),
                None,
                0,
                &mut missing,
            );
            capture.observe(
                text_sample(Kind::Key(Key::Character('A'), 0), 10),
                &text_context(Some("")),
                None,
                10,
                &mut missing,
            );
            let ctx = text_context(Some("a"));
            let committed = capture.observe(
                text_sample(Kind::Focus, 15),
                &ctx,
                Some(next),
                15,
                &mut missing,
            );
            assert!(
                committed.is_some(),
                "already-read text must survive the queued boundary"
            );
            let (input, value) = committed.unwrap();
            let mut timeline = Timeline::new("boundary".into(), "owner".into(), "transport".into());
            push(
                &mut timeline,
                input,
                &ctx,
                "typeText",
                json!({"text":value}),
            );
            let other = Context {
                focus: "field-b".into(),
                ..text_context(Some(""))
            };
            capture.observe(
                Raw {
                    focus: 4,
                    ..text_sample(Kind::Focus, 21)
                },
                &other,
                None,
                21,
                &mut missing,
            );
            push(
                &mut timeline,
                next,
                &other,
                "click",
                json!({"x":30,"y":40,"button":"left","count":1}),
            );
            timeline.finish("requested");
            assert_eq!(timeline.actions[0]["actionType"], "typeText");
            assert_eq!(timeline.actions[0]["payload"]["text"], "a");
            assert_eq!(timeline.actions[1]["actionType"], "click");
            assert_eq!(missing, 0);
            assert!(capture.pending.is_none());
        }
    }

    #[test]
    fn composition_pause_waits_for_commit_without_false_loss() {
        let mut capture = TextCapture::default();
        let mut missing = 0;
        capture.observe(
            text_sample(Kind::Focus, 0),
            &text_context(Some("")),
            None,
            0,
            &mut missing,
        );
        capture.observe(
            text_sample(Kind::Key(Key::Character('N'), 0), 10),
            &text_context(Some("")),
            None,
            10,
            &mut missing,
        );
        assert!(capture
            .observe(
                text_sample(Kind::Focus, 5000),
                &text_context(Some("")),
                None,
                5000,
                &mut missing
            )
            .is_none());
        assert_eq!(missing, 0);
        let (_, value) = capture
            .observe(
                text_sample(Kind::Key(Key::Character(' '), 0), 6000),
                &text_context(Some("你好")),
                None,
                6000,
                &mut missing,
            )
            .unwrap();
        assert_eq!(value, "你好");
        assert!(capture
            .observe(
                text_sample(Kind::Focus, 7000),
                &text_context(Some("你好")),
                None,
                7000,
                &mut missing
            )
            .is_none());
        assert!(capture.pending.is_none());
        assert_eq!(missing, 0);
    }

    #[test]
    fn queued_ime_commit_is_consumed_once_without_a_phantom_pending_key() {
        let mut capture = TextCapture::default();
        let mut missing = 0;
        capture.observe(
            text_sample(Kind::Focus, 0),
            &text_context(Some("")),
            None,
            0,
            &mut missing,
        );
        capture.observe(
            text_sample(Kind::Key(Key::Character('N'), 0), 10),
            &text_context(Some("")),
            None,
            10,
            &mut missing,
        );
        assert!(capture
            .observe(
                text_sample(Kind::Focus, 2000),
                &text_context(Some("你")),
                Some(text_sample(Kind::Key(Key::Character(' '), 0), 2001)),
                2000,
                &mut missing
            )
            .is_none());
        let (_, value) = capture
            .observe(
                text_sample(Kind::Key(Key::Character(' '), 0), 2001),
                &text_context(Some("你")),
                None,
                2001,
                &mut missing,
            )
            .unwrap();
        assert_eq!(value, "你");
        assert!(capture.pending.is_none());
        assert_eq!(missing, 0);
    }

    #[test]
    fn paste_seen_before_queued_key_keeps_the_pre_paste_baseline() {
        let mut capture = TextCapture::default();
        let mut missing = 0;
        capture.observe(
            text_sample(Kind::Focus, 0),
            &text_context(Some("")),
            None,
            0,
            &mut missing,
        );
        // The UIA poll and modifier key can both see the new value before V.
        for (kind, next_kind) in [
            (Kind::Focus, Kind::Key(Key::Character('¢'), 0)),
            (
                Kind::Key(Key::Character('¢'), 0),
                Kind::Key(Key::Character('V'), 2),
            ),
        ] {
            assert!(capture
                .observe(
                    text_sample(kind, 10),
                    &text_context(Some("好")),
                    Some(text_sample(next_kind, 11)),
                    10,
                    &mut missing
                )
                .is_none());
        }
        let (_, value) = capture
            .observe(
                text_sample(Kind::Key(Key::Character('V'), 2), 11),
                &text_context(Some("好")),
                None,
                11,
                &mut missing,
            )
            .unwrap();
        assert_eq!(value, "好");
        assert_eq!(missing, 0);
        assert!(capture
            .observe(
                text_sample(Kind::Focus, 20),
                &text_context(Some("好")),
                None,
                20,
                &mut missing
            )
            .is_none());
    }

    #[test]
    fn cancelled_composition_is_not_missing_but_unreadable_input_is() {
        let mut capture = TextCapture::default();
        let mut missing = 0;
        capture.observe(
            text_sample(Kind::Focus, 0),
            &text_context(Some("已有")),
            None,
            0,
            &mut missing,
        );
        capture.observe(
            text_sample(Kind::Key(Key::Character('N'), 0), 10),
            &text_context(Some("已有")),
            None,
            10,
            &mut missing,
        );
        capture.observe(
            text_sample(Kind::Key(Key::Named("Escape"), 0), 5000),
            &text_context(Some("已有")),
            None,
            5000,
            &mut missing,
        );
        assert!(capture.pending.is_none());
        assert_eq!(missing, 0);
        capture.observe(
            text_sample(Kind::Key(Key::Character('N'), 0), 6000),
            &text_context(None),
            None,
            6000,
            &mut missing,
        );
        capture.observe(
            text_sample(Kind::Focus, 8000),
            &text_context(None),
            None,
            8000,
            &mut missing,
        );
        assert_eq!(missing, 1);
        assert!(capture.pending.is_none());
    }

    #[test]
    fn untriggered_value_changes_and_focus_changes_do_not_become_typed_text() {
        let mut capture = TextCapture::default();
        let mut missing = 0;
        capture.observe(
            text_sample(Kind::Focus, 0),
            &text_context(Some("")),
            None,
            0,
            &mut missing,
        );
        assert!(capture
            .observe(
                text_sample(Kind::Focus, 10),
                &text_context(Some("app update")),
                None,
                10,
                &mut missing
            )
            .is_none());
        capture.observe(
            text_sample(Kind::Key(Key::Character('N'), 0), 20),
            &text_context(Some("app update")),
            None,
            20,
            &mut missing,
        );
        let other = Context {
            focus: "field-b".into(),
            ..text_context(Some("other field"))
        };
        assert!(capture
            .observe(text_sample(Kind::Focus, 30), &other, None, 30, &mut missing)
            .is_none());
        assert_eq!(missing, 1);
    }
}
