//! Explicit Windows human demonstrations. Hook callbacks only snapshot native
//! identity/input and try_send into a bounded queue. Inspired by Screenpipe's
//! windows.rs at b23efb5d876c04865e451155589783ad7911de4c; see product notices.
use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    teaching::{self, Action, Source, Timeline},
    tool::{Tool, ToolDef, ToolRegistry},
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[cfg(test)]
use windows::core::w;
use windows::{
    core::Interface,
    Win32::{
        Foundation::*,
        System::{Com::*, Threading::*},
        UI::{Accessibility::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
};

#[path = "teaching_evidence.rs"]
mod evidence;
use cua_driver_core::perception_client::PerceptionClientHandle;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone, Copy)]
enum Kind {
    Key(u32, u8),
    Click(i32, i32, &'static str),
    Scroll(i32, i32, i32, bool),
    Focus,
}
#[derive(Clone, Copy)]
struct Raw {
    window: u64,
    focus: u64,
    pid: u32,
    at: u64,
    kind: Kind,
}
#[derive(Default)]
struct Counters {
    dropped: AtomicU64,
}
struct HookState {
    tx: SyncSender<Raw>,
    accepting: Arc<AtomicBool>,
    counters: Arc<Counters>,
    excluded: Vec<u32>,
}
thread_local! { static HOOK: RefCell<Option<HookState>> = const { RefCell::new(None) }; }

fn identity(kind: Kind) -> Option<Raw> {
    unsafe {
        let window = match kind {
            Kind::Click(x, y, _) | Kind::Scroll(x, y, _, _) => {
                GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT)
            }
            _ => GetForegroundWindow(),
        };
        if window.0.is_null() {
            return None;
        }
        let mut pid = 0;
        let tid = GetWindowThreadProcessId(window, Some(&mut pid));
        let mut gui = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(tid, &mut gui).is_err() {
            return None;
        }
        Some(Raw {
            window: window.0 as u64,
            focus: gui.hwndFocus.0 as u64,
            pid,
            at: now(),
            kind,
        })
    }
}
fn enqueue(kind: Kind) {
    HOOK.with(|cell| {
        let Ok(guard) = cell.try_borrow() else {
            return;
        };
        let Some(state) = guard.as_ref() else {
            return;
        };
        if !state.accepting.load(Ordering::Acquire) {
            return;
        }
        let Some(raw) = identity(kind) else {
            state.counters.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if state.excluded.contains(&raw.pid) {
            return;
        }
        if state.tx.try_send(raw).is_err() {
            state.counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
    });
}
unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let key = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let mut mods = 0;
        for (vk, bit) in [
            (VK_SHIFT, 1),
            (VK_CONTROL, 2),
            (VK_MENU, 4),
            (VK_LWIN, 8),
            (VK_RWIN, 8),
        ] {
            if GetAsyncKeyState(vk.0 as i32) < 0 {
                mods |= bit;
            }
        }
        enqueue(Kind::Key(key.vkCode, mods));
    }
    CallNextHookEx(None, code, wparam, lparam)
}
unsafe extern "system" fn mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let m = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let kind = match wparam.0 as u32 {
            WM_LBUTTONDOWN => Some(Kind::Click(m.pt.x, m.pt.y, "left")),
            WM_RBUTTONDOWN => Some(Kind::Click(m.pt.x, m.pt.y, "right")),
            WM_MBUTTONDOWN => Some(Kind::Click(m.pt.x, m.pt.y, "middle")),
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => Some(Kind::Scroll(
                m.pt.x,
                m.pt.y,
                (m.mouseData >> 16) as i16 as i32,
                wparam.0 as u32 == WM_MOUSEHWHEEL,
            )),
            _ => None,
        };
        if let Some(kind) = kind {
            enqueue(kind);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

struct Hooks {
    thread_id: u32,
    done: mpsc::Receiver<()>,
    accepting: Arc<AtomicBool>,
}
impl Hooks {
    fn start(
        tx: SyncSender<Raw>,
        accepting: Arc<AtomicBool>,
        counters: Arc<Counters>,
        excluded: Vec<u32>,
    ) -> Result<Self, String> {
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let (done_tx, done) = mpsc::sync_channel(1);
        let active = accepting.clone();
        thread::Builder::new()
            .name("cua-teaching-hooks".into())
            .spawn(move || {
                unsafe {
                    let mut message = MSG::default();
                    let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
                    HOOK.with(|s| {
                        *s.borrow_mut() = Some(HookState {
                            tx,
                            accepting: active.clone(),
                            counters,
                            excluded,
                        })
                    });
                    let keyboard_hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), None, 0);
                    let mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), None, 0);
                    match (keyboard_hook, mouse_hook) {
                        (Ok(k), Ok(m)) => {
                            let _ = ready_tx.send(Ok(GetCurrentThreadId()));
                            while active.load(Ordering::Acquire)
                                && GetMessageW(&mut message, None, 0, 0).0 > 0
                            {
                                let _ = TranslateMessage(&message);
                                DispatchMessageW(&message);
                            }
                            let _ = UnhookWindowsHookEx(k);
                            let _ = UnhookWindowsHookEx(m);
                        }
                        (k, m) => {
                            if let Ok(k) = k {
                                let _ = UnhookWindowsHookEx(k);
                            }
                            if let Ok(m) = m {
                                let _ = UnhookWindowsHookEx(m);
                            }
                            let _ = ready_tx.send(Err("teaching_hook_creation_failed".into()));
                        }
                    }
                    HOOK.with(|s| s.borrow_mut().take());
                }
                let _ = done_tx.send(());
            })
            .map_err(|_| "teaching_thread_failed")?;
        match ready.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                done,
                accepting,
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                accepting.store(false, Ordering::Release);
                Err("teaching_hook_timeout".into())
            }
        }
    }
    fn stop(&self) {
        self.accepting.store(false, Ordering::Release);
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        let _ = self.done.recv_timeout(Duration::from_millis(500));
    }
}
impl Drop for Hooks {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Default)]
struct Context {
    safe: bool,
    sensitive: bool,
    // Same UIA control identity for keyboard, polling and click observations.
    focus: String,
    // Click coalescing must not redefine text identity.
    click_target: String,
    // RuntimeId is only for screenshot target proof, including HWND-less UIA controls.
    runtime_id: Option<Vec<i32>>,
    element: Value,
    value: Option<String>,
    rect: [i32; 4],
}
struct ContextRequest {
    raw: Raw,
    reply: SyncSender<Context>,
}
/// Exactly one COM worker per process. A hung provider occupies this worker;
/// subsequent calls fail closed instead of spawning more stuck COM threads.
fn context_worker() -> &'static SyncSender<ContextRequest> {
    static WORKER: OnceLock<SyncSender<ContextRequest>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<ContextRequest>(1);
        thread::Builder::new()
            .name("cua-teaching-uia".into())
            .spawn(move || unsafe {
                let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
                let automation: Option<IUIAutomation> = if initialized {
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()
                } else {
                    None
                };
                for request in rx {
                    let context = automation
                        .as_ref()
                        .and_then(|a| read_context(a, request.raw).ok())
                        .unwrap_or_default();
                    let _ = request.reply.try_send(context);
                }
                if initialized {
                    CoUninitialize();
                }
            })
            .expect("create teaching UIA worker");
        tx
    })
}
fn context_target_matches(raw: Raw) -> bool {
    let click = matches!(raw.kind, Kind::Click(..));
    identity(if click { raw.kind } else { Kind::Focus }).is_some_and(|current| {
        current.window == raw.window
            && current.pid == raw.pid
            && (click || current.focus == raw.focus)
    })
}
fn text_context_identity(raw: Raw, native_handle: u64, control: &str) -> String {
    // NativeWindowHandle=0 is valid for virtual controls. Never substitute the
    // host's focused HWND only on the keyboard path, or the identities diverge.
    format!("{}:{}:{native_handle}:{control}", raw.pid, raw.window)
}
unsafe fn read_context(automation: &IUIAutomation, raw: Raw) -> windows::core::Result<Context> {
    // Like the old point-based click enrichment, a click may itself change
    // focus. Resolve its target by position, not by the pre-click focused HWND.
    if !matches!(raw.kind, Kind::Click(..)) && now().saturating_sub(raw.at) > 250 {
        return Ok(Context::default());
    }
    if !context_target_matches(raw) {
        return Ok(Context::default());
    }
    let element = match raw.kind {
        Kind::Click(x, y, _) => automation.ElementFromPoint(POINT { x, y })?,
        _ => automation.GetFocusedElement()?,
    };
    if element.CurrentProcessId()? as u32 != raw.pid {
        return Ok(Context::default());
    }
    // Password proof precedes every content-bearing property read.
    if element.CurrentIsPassword()?.as_bool() {
        return Ok(Context {
            sensitive: true,
            ..Default::default()
        });
    }
    let name = element.CurrentName()?.to_string();
    let identifier = element.CurrentAutomationId()?.to_string();
    let rect = element.CurrentBoundingRectangle()?;
    let native_handle = element
        .CurrentNativeWindowHandle()
        .map(|h| h.0 as u64)
        .unwrap_or(0);
    let control = format!(
        "{}:{}:{}:{}:{}",
        identifier,
        rect.left,
        rect.top,
        element.CurrentControlType()?.0,
        name
    );
    let focus = text_context_identity(raw, native_handle, &control);
    let value = element
        .GetCurrentPattern(UIA_ValuePatternId)
        .ok()
        .and_then(|p| p.cast::<IUIAutomationValuePattern>().ok())
        .and_then(|p| p.CurrentValue().ok())
        .map(|v| v.to_string())
        .filter(|s| s.len() <= 16384);
    // The call may have raced a focus/window change while a provider was slow.
    if !context_target_matches(raw) {
        return Ok(Context::default());
    }
    Ok(Context {
        safe: true,
        sensitive: false,
        focus,
        click_target: format!("{native_handle}:{control}"),
        runtime_id: evidence::runtime_id(&element).ok(),
        element: json!({"role":element.CurrentLocalizedControlType()?.to_string(),"title":name,"identifier":identifier}),
        value,
        rect: [rect.left, rect.top, rect.right, rect.bottom],
    })
}
fn context(raw: Raw) -> Context {
    if !matches!(raw.kind, Kind::Click(..)) && now().saturating_sub(raw.at) > 250 {
        return Context::default();
    }
    let (tx, rx) = mpsc::sync_channel(1);
    if context_worker()
        .try_send(ContextRequest { raw, reply: tx })
        .is_err()
    {
        return Context::default();
    }
    rx.recv_timeout(Duration::from_millis(750))
        .unwrap_or_default()
}

struct Live {
    timeline: Arc<Mutex<Timeline>>,
    accepting: Arc<AtomicBool>,
    reason: Arc<Mutex<String>>,
    done: mpsc::Receiver<()>,
    evidence: Arc<evidence::EvidenceLog>,
    cancelled: bool,
}
impl Live {
    fn stop(&mut self, reason: &str) {
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
    live: Option<Live>,
    perception: PerceptionClientHandle,
    exclusion: Arc<teaching::TeachingState>,
}

fn key_name(key: u32) -> Option<String> {
    Some(match key {
        8 => "Backspace".into(),
        9 => "Tab".into(),
        13 => "Enter".into(),
        27 => "Escape".into(),
        33 => "PageUp".into(),
        34 => "PageDown".into(),
        35 => "End".into(),
        36 => "Home".into(),
        37 => "Left".into(),
        38 => "Up".into(),
        39 => "Right".into(),
        40 => "Down".into(),
        46 => "Delete".into(),
        112..=135 => format!("F{}", key - 111),
        48..=90 => char::from_u32(key)?.to_string(),
        32 => "Space".into(),
        _ => return None,
    })
}
fn modifiers(mods: u8) -> Vec<&'static str> {
    [(1, "shift"), (2, "ctrl"), (4, "alt"), (8, "meta")]
        .iter()
        .filter_map(|(bit, name)| (mods & bit != 0).then_some(*name))
        .collect()
}
fn base(raw: Raw, ctx: &Context) -> Value {
    // No title reads from another process in the input callback. PID is a
    // stable safe app attribution even if semantic enrichment is unavailable.
    let process = process_name(raw.pid);
    let title = if ctx.safe {
        unsafe {
            let mut title = [0u16; 512];
            let n = GetWindowTextW(HWND(raw.window as *mut _), &mut title);
            Some(String::from_utf16_lossy(&title[..n.max(0) as usize]))
        }
    } else {
        None
    };
    json!({"app":process.clone().unwrap_or_else(||format!("pid:{}",raw.pid)),"processName":process,"windowTitle":title,"url":null,"element":if ctx.safe {ctx.element.clone()} else {Value::Null}})
}
fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut path = [0u16; 1024];
        let mut size = path.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(path.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        Some(
            String::from_utf16_lossy(&path[..size as usize])
                .rsplit('\\')
                .next()?
                .to_owned(),
        )
    }
}
fn push(t: &mut Timeline, raw: Raw, ctx: &Context, kind: &'static str, extra: Value) {
    let mut payload = base(raw, ctx);
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
struct TextCapture {
    baseline: Option<(u64, String, String)>,
    pending: Option<Raw>,
}
fn is_text_input(kind: Kind) -> bool {
    matches!(kind, Kind::Key(key, mods) if key == 229
        || (mods & 14 == 0
            && !matches!(key,8|9|13|16..=18|27|33..=40|46|91..=92|112..=135|160..=165))
        || (mods & 2 != 0 && key == 86))
}
impl TextCapture {
    fn observe(
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
                            && matches!(next.kind, Kind::Key(16..=18|91..=92|160..=165, _))))
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
                } else if matches!(raw.kind, Kind::Key(27, _)) {
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

fn run(
    rx: mpsc::Receiver<Raw>,
    hooks: Hooks,
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
            if !crate::diagnostics::desktop_state().input_desktop_is_default() {
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
        let raw = match received.or_else(|| identity(Kind::Focus)) {
            Some(r) => r,
            None => continue,
        };
        if received.is_some() {
            last_input_at = raw.at;
        }
        // Candidate popups belong to an IME, not to the demonstrated workflow.
        // Keep the business field baseline while the user chooses a candidate.
        let candidate = unsafe {
            let mut class = [0u16; 128];
            let n = GetClassNameW(HWND(raw.window as *mut _), &mut class);
            let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
            class == "IME" || class == "MSCTFIME UI" || class.contains("IME.UIManager")
        };
        if candidate {
            continue;
        }
        if excluded.contains(&raw.pid) {
            text.baseline = None;
            text.pending = None;
            continue;
        }
        let ctx = context(raw);
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
                let app = process_name(raw.pid);
                push(
                    &mut t,
                    raw,
                    &Context::default(),
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
                        &Context::default(),
                        "click",
                        json!({"x":x,"y":y,"button":button,"count":1}),
                    ),
                    Kind::Scroll(x, y, delta, horizontal) => push(
                        &mut t,
                        raw,
                        &Context::default(),
                        "scroll",
                        json!({"x":x,"y":y,"deltaX":if horizontal {delta}else{0},"deltaY":if horizontal {0}else{delta}}),
                    ),
                    _ => push(
                        &mut t,
                        raw,
                        &Context::default(),
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
                if matches!(key,16..=18|91..=92|160..=165) {
                    continue;
                }
                if mods & 14 != 0 && key != 229 {
                    if let Some(name) = key_name(key) {
                        push(
                            &mut t,
                            raw,
                            &ctx,
                            "hotkey",
                            json!({"keyName":name,"modifiers":modifiers(mods)}),
                        );
                    }
                } else if matches!(key,8|9|13|27|33..=40|46|112..=135) {
                    if let Some(name) = key_name(key) {
                        push(
                            &mut t,
                            raw,
                            &ctx,
                            "keyPress",
                            json!({"keyCode":key,"keyName":name,"modifiers":modifiers(mods)}),
                        );
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

fn persist_final(t: &mut Timeline, evidence: &evidence::EvidenceLog) {
    let Some(output) = t.output.clone() else {
        return;
    };
    let mut data = t.snapshot(0, 0);
    data["evidence"] = evidence.snapshot(0, 0);
    if output.state(&data).is_err() {
        t.output_errors += 1;
        return;
    }
    if data["complete"] == true
        && output
            .write_json(
                "complete.json",
                &json!({
                    "cutoff_sequence": data["cutoff_sequence"],
                    "evidence_complete": data["evidence"]["complete"],
                    "evidence_cutoff_sequence": data["evidence"]["cutoff_sequence"]
                }),
            )
            .is_err()
    {
        t.output_errors += 1;
        data["complete"] = json!(false);
        data["diagnostics"]["output_errors"] = json!(t.output_errors);
        let _ = output.state(&data);
    }
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
            let desktop = crate::diagnostics::desktop_state();
            if !desktop.input_desktop_is_default() || desktop.session_id.is_none_or(|id| id == 0) {
                return Err("teaching_desktop_unavailable".into());
            }
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
                match Hooks::start(tx, accepting.clone(), counters.clone(), excluded.clone()) {
                    Ok(h) => h,
                    Err(e) => {
                        state.finish(&e);
                        let _ = output.state(&state.snapshot(0, 0));
                        return Err(e);
                    }
                };
            unsafe {
                state.double_click_ms = GetDoubleClickTime().into();
                state.double_click_distance =
                    (GetSystemMetrics(SM_CXDOUBLECLK).max(GetSystemMetrics(SM_CYDOUBLECLK)) / 2)
                        .into();
            }
            let timeline = Arc::new(Mutex::new(state));
            let reason = Arc::new(Mutex::new("requested".into()));
            let evidence = Arc::new(evidence::EvidenceLog::new(self.perception.clone(), output));
            let (done_tx, done) = mpsc::sync_channel(1);
            let (t, a, r, e) = (
                timeline.clone(),
                accepting.clone(),
                reason.clone(),
                evidence.clone(),
            );
            thread::Builder::new()
                .name("cua-teaching-events".into())
                .spawn(move || {
                    run(rx, hooks, t, a, r, counters, excluded, seconds, e);
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
        result.content[0] = cua_driver_core::protocol::Content::text(data.to_string());
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
pub fn register_into(registry: &mut ToolRegistry, perception: PerceptionClientHandle) {
    let manager = Arc::new(Mutex::new(Manager {
        live: None,
        perception,
        exclusion: registry.teaching_state(),
    }));
    let weak = Arc::downgrade(&manager);
    registry.retain_session_end_hook(cua_driver_core::session::register_scoped_session_end_hook(
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
    fn hwndless_control_keeps_text_identity_across_keyboard_click_and_poll() {
        let mut capture = TextCapture::default();
        let mut missing = 0;
        let sample = |kind, at, value: &str| {
            let raw = Raw {
                focus: 99,
                ..text_sample(kind, at)
            };
            let ctx = Context {
                focus: text_context_identity(raw, 0, "virtual-edit-a"),
                ..text_context(Some(value))
            };
            (raw, ctx)
        };
        for (kind, at) in [(Kind::Focus, 0), (Kind::Key(65, 0), 10)] {
            let (raw, ctx) = sample(kind, at, "");
            capture.observe(raw, &ctx, None, at, &mut missing);
        }
        let (click, clicked) = sample(Kind::Click(30, 40, "left"), 20, "a");
        let committed = capture.observe(click, &clicked, None, 20, &mut missing);
        assert!(
            committed.is_some(),
            "same HWND-less input must retain the already-read a; missing={missing}"
        );
        assert_eq!(committed.unwrap().1, "a");
        let (poll, polled) = sample(Kind::Focus, 30, "a");
        assert!(capture
            .observe(poll, &polled, None, 30, &mut missing)
            .is_none());
        let (key, typed) = sample(Kind::Key(66, 0), 40, "ab");
        assert_eq!(
            capture
                .observe(key, &typed, None, 40, &mut missing)
                .unwrap()
                .1,
            "b"
        );
        assert_eq!(missing, 0);
    }

    #[test]
    fn hwndless_controls_sharing_a_host_keep_distinct_text_identities() {
        let raw = Raw {
            focus: 99,
            ..text_sample(Kind::Focus, 0)
        };
        assert_ne!(
            text_context_identity(raw, 0, "virtual-a"),
            text_context_identity(raw, 0, "virtual-b")
        );
        assert_eq!(
            text_context_identity(raw, 0, "virtual-a"),
            text_context_identity(
                Raw {
                    focus: 100,
                    kind: Kind::Click(30, 40, "left"),
                    ..raw
                },
                0,
                "virtual-a"
            ),
        );
        assert_ne!(
            text_context_identity(raw, 0, "virtual-a"),
            text_context_identity(Raw { window: 9, ..raw }, 0, "virtual-a")
        );
        let mut capture = TextCapture::default();
        let mut missing = 0;
        let a = Context {
            focus: text_context_identity(raw, 0, "virtual-a"),
            ..text_context(Some(""))
        };
        let b = Context {
            focus: text_context_identity(raw, 0, "virtual-b"),
            ..text_context(Some("other field"))
        };
        capture.observe(raw, &a, None, 0, &mut missing);
        capture.observe(
            Raw {
                kind: Kind::Key(65, 0),
                at: 10,
                ..raw
            },
            &a,
            None,
            10,
            &mut missing,
        );
        assert!(capture
            .observe(Raw { at: 20, ..raw }, &b, None, 20, &mut missing)
            .is_none());
        assert_eq!(missing, 1, "unknown A text must not be fabricated from B");
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
                ..text_sample(Kind::Key(66, 0), 20)
            },
            Raw {
                window: 9,
                ..text_sample(Kind::Key(66, 0), 20)
            },
            Raw {
                pid: 9,
                ..text_sample(Kind::Key(66, 0), 20)
            },
            text_sample(Kind::Key(9, 0), 20),
            text_sample(Kind::Key(65, 2), 20),
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
                text_sample(Kind::Key(65, 0), 10),
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
            text_sample(Kind::Key(78, 0), 10),
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
                text_sample(Kind::Key(32, 0), 6000),
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
            text_sample(Kind::Key(78, 0), 10),
            &text_context(Some("")),
            None,
            10,
            &mut missing,
        );
        assert!(capture
            .observe(
                text_sample(Kind::Focus, 2000),
                &text_context(Some("你")),
                Some(text_sample(Kind::Key(32, 0), 2001)),
                2000,
                &mut missing
            )
            .is_none());
        let (_, value) = capture
            .observe(
                text_sample(Kind::Key(32, 0), 2001),
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
            (Kind::Focus, Kind::Key(162, 0)),
            (Kind::Key(162, 0), Kind::Key(86, 2)),
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
                text_sample(Kind::Key(86, 2), 11),
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
            text_sample(Kind::Key(78, 0), 10),
            &text_context(Some("已有")),
            None,
            10,
            &mut missing,
        );
        capture.observe(
            text_sample(Kind::Key(27, 0), 5000),
            &text_context(Some("已有")),
            None,
            5000,
            &mut missing,
        );
        assert!(capture.pending.is_none());
        assert_eq!(missing, 0);
        capture.observe(
            text_sample(Kind::Key(78, 0), 6000),
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
            text_sample(Kind::Key(78, 0), 20),
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

    struct Fixture {
        tid: u32,
        label: u64,
        ack: mpsc::Receiver<()>,
        thread: Option<thread::JoinHandle<()>>,
    }
    impl Fixture {
        fn new() -> Self {
            let (ready_tx, ready) = mpsc::sync_channel(1);
            let (ack_tx, ack) = mpsc::sync_channel(1);
            let thread = thread::spawn(move || unsafe {
                let window = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("CUA password documentation fixture"),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    100,
                    100,
                    600,
                    320,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let edit = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("EDIT"),
                    w!(""),
                    WS_CHILD | WS_VISIBLE | WS_BORDER,
                    20,
                    40,
                    520,
                    60,
                    window,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let password = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("EDIT"),
                    w!("DO_NOT_RETAIN_PASSWORD"),
                    WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_PASSWORD as u32),
                    20,
                    130,
                    520,
                    40,
                    window,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let label = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("你好，世界"),
                    WS_CHILD | WS_VISIBLE,
                    20,
                    190,
                    520,
                    40,
                    window,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let second = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("CUA teaching second window"),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    740,
                    100,
                    400,
                    200,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let second_edit = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("EDIT"),
                    w!(""),
                    WS_CHILD | WS_VISIBLE | WS_BORDER,
                    20,
                    40,
                    340,
                    60,
                    second,
                    None,
                    None,
                    None,
                )
                .unwrap();
                assert!(crate::input::force_foreground_attached(window));
                let _ = SetFocus(edit);
                ready_tx
                    .send((GetCurrentThreadId(), label.0 as u64))
                    .unwrap();
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    match msg.message {
                        x if x == WM_APP + 1 => {
                            assert!(crate::input::force_foreground_attached(window));
                            let _ = SetFocus(edit);
                            let _ = SendMessageW(
                                edit,
                                WM_SETTEXT,
                                WPARAM(0),
                                LPARAM(w!("你好，世界").as_ptr() as isize),
                            );
                            let _ = ack_tx.send(());
                        }
                        x if x == WM_APP + 2 => {
                            assert!(crate::input::force_foreground_attached(window));
                            let _ = SetFocus(password);
                            let _ = ack_tx.send(());
                        }
                        x if x == WM_APP + 3 => {
                            let _ = DestroyWindow(window);
                            let _ = DestroyWindow(second);
                            break;
                        }
                        x if x == WM_APP + 4 => {
                            assert!(crate::input::force_foreground_attached(second));
                            let _ = SetFocus(second_edit);
                            let _ = ack_tx.send(());
                        }
                        _ => {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                }
            });
            let (tid, label) = ready.recv_timeout(Duration::from_secs(3)).unwrap();
            Self {
                tid,
                label,
                ack,
                thread: Some(thread),
            }
        }
        fn command(&self, command: u32) {
            unsafe {
                PostThreadMessageW(self.tid, WM_APP + command, WPARAM(0), LPARAM(0)).unwrap();
            }
            self.ack.recv_timeout(Duration::from_secs(2)).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe {
                let _ = PostThreadMessageW(self.tid, WM_APP + 3, WPARAM(0), LPARAM(0));
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    #[test]
    #[ignore = "interactive Windows desktop; fixture-owned UIA queries"]
    fn native_teaching_click_context_survives_focus_change() {
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        let fixture = Fixture::new();
        let before = identity(Kind::Focus).unwrap();
        let mut rect = RECT::default();
        unsafe {
            GetWindowRect(HWND(fixture.label as *mut _), &mut rect).unwrap();
        }
        let mut click = identity(Kind::Click(
            (rect.left + rect.right) / 2,
            (rect.top + rect.bottom) / 2,
            "left",
        ))
        .unwrap();
        fixture.command(2); // Focus changes to a password field; clicked label is not a password.
        assert_ne!(identity(Kind::Focus).unwrap().focus, click.focus);
        click.at = now() - 1000; // No arbitrary 250ms cutoff for point-based clicks.
        let context = super::context(click);
        assert!(context.safe && !context.sensitive);
        assert_eq!(context.element["title"], "你好，世界");
        assert!(
            !super::context(Raw {
                at: now(),
                ..before
            })
            .safe
        );
        assert!(
            !super::context(Raw {
                pid: click.pid + 1,
                ..click
            })
            .safe
        );
        let password = super::context(identity(Kind::Focus).unwrap());
        assert!(password.sensitive && !password.safe && password.value.is_none());
        fixture.command(4);
        assert!(
            !super::context(Raw {
                window: identity(Kind::Focus).unwrap().window,
                ..click
            })
            .safe
        );
    }

    #[test]
    #[ignore = "interactive Windows desktop; fixture-owned focus and UIA"]
    fn native_teaching_double_click_uses_target_identity_after_focus_change() {
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        let fixture = Fixture::new();
        let edit = identity(Kind::Focus).unwrap().focus;
        let mut rect = RECT::default();
        unsafe {
            GetWindowRect(HWND(edit as *mut _), &mut rect).unwrap();
        }
        let (x, y) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        fixture.command(2);
        let first = identity(Kind::Click(x, y, "left")).unwrap();
        fixture.command(1);
        let first_context = context(first);
        let second = Raw {
            at: first.at + 100,
            ..identity(Kind::Click(x, y, "left")).unwrap()
        };
        let second_context = context(second);
        assert_ne!(first.focus, second.focus);
        assert!(first_context.safe && second_context.safe);
        let mut timeline = Timeline::new("double".into(), "owner".into(), "transport".into());
        for (raw, ctx) in [(first, first_context), (second, second_context)] {
            push(
                &mut timeline,
                raw,
                &ctx,
                "click",
                json!({"x":x,"y":y,"button":"left","count":1}),
            );
        }
        timeline.finish("requested");
        assert_eq!(timeline.actions.len(), 1, "{:?}", timeline.actions);
        assert_eq!(timeline.actions[0]["payload"]["count"], 2);
    }

    /// Native UIA and the real event-processing channel, with fixture-owned
    /// semantic changes. This is not evidence of physical keyboard/IME input.
    #[test]
    #[ignore = "interactive Windows desktop; run products/cua-win/verify-teaching.ps1"]
    fn native_teaching_uia_text_privacy_and_event_pipeline() {
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        let fixture = Fixture::new();
        thread::sleep(Duration::from_millis(200));
        let initial = identity(Kind::Focus).unwrap();
        assert_eq!(
            initial.pid,
            std::process::id(),
            "fixture must own foreground"
        );
        let first = context(initial);
        assert!(first.safe, "fixture UIA context must be available");
        assert_eq!(first.value.as_deref(), Some(""));
        let mut rect = RECT::default();
        unsafe {
            GetWindowRect(HWND(initial.focus as *mut _), &mut rect).unwrap();
        }
        let (x, y) = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        let (tx, rx) = mpsc::sync_channel(256);
        let accepting = Arc::new(AtomicBool::new(true));
        let counters = Arc::new(Counters::default());
        let hooks = Hooks::start(tx.clone(), accepting.clone(), counters.clone(), vec![]).unwrap();
        let timeline = Arc::new(Mutex::new(Timeline::new(
            "native-pipeline".into(),
            "owner".into(),
            "transport".into(),
        )));
        let reason = Arc::new(Mutex::new("requested".into()));
        let (done_tx, done) = mpsc::sync_channel(1);
        let output_parent = tempfile::tempdir().unwrap();
        let output = Arc::new(teaching::SessionOutput::create(output_parent.path()).unwrap());
        timeline.lock().unwrap().output = Some(output.clone());
        let evidence = Arc::new(evidence::EvidenceLog::new(
            PerceptionClientHandle::fixed(
                cua_driver_core::perception_client::PerceptionClient::unavailable(),
            ),
            output,
        ));
        let (t, a, r, e) = (
            timeline.clone(),
            accepting.clone(),
            reason.clone(),
            evidence.clone(),
        );
        let worker = thread::spawn(move || {
            run(rx, hooks, t, a, r, counters, vec![], 10, e);
            let _ = done_tx.send(());
        });
        let mut live = Live {
            timeline: timeline.clone(),
            accepting,
            reason,
            done,
            evidence,
            cancelled: false,
        };
        thread::sleep(Duration::from_millis(250));
        tx.send(identity(Kind::Key(229, 0)).unwrap()).unwrap();
        thread::sleep(Duration::from_millis(100));
        fixture.command(1);
        thread::sleep(Duration::from_millis(250));
        for kind in [
            Kind::Click(x, y, "left"),
            Kind::Click(x, y, "left"),
            Kind::Click(x, y, "right"),
            Kind::Scroll(x, y, 120, false),
            Kind::Scroll(x, y, 120, false),
            Kind::Key(83, 2),
        ] {
            tx.send(identity(kind).unwrap()).unwrap();
            thread::sleep(Duration::from_millis(80));
        }
        fixture.command(2);
        thread::sleep(Duration::from_millis(100));
        let password = context(identity(Kind::Focus).unwrap());
        assert!(password.sensitive && !password.safe && password.value.is_none());
        tx.send(identity(Kind::Key(65, 0)).unwrap()).unwrap();
        thread::sleep(Duration::from_millis(250));
        fixture.command(4);
        thread::sleep(Duration::from_millis(150));
        tx.send(identity(Kind::Key(13, 0)).unwrap()).unwrap();
        let mut stale = identity(Kind::Key(66, 0)).unwrap();
        stale.at = now() - 1000;
        tx.send(stale).unwrap();
        thread::sleep(Duration::from_millis(150));
        live.stop("requested");
        worker.join().unwrap();
        let state = timeline.lock().unwrap().snapshot(0, 100);
        let actions = state["actions"].as_array().unwrap();
        assert!(
            actions
                .iter()
                .any(|a| a["actionType"] == "typeText" && a["payload"]["text"] == "你好，世界"),
            "{state}"
        );
        assert!(
            actions
                .iter()
                .any(|a| a["actionType"] == "click" && a["payload"]["count"] == 2),
            "{state}"
        );
        assert!(
            actions
                .iter()
                .any(|a| a["actionType"] == "scroll" && a["payload"]["deltaY"] == 240),
            "{state}"
        );
        assert!(
            actions.iter().any(|a| a["actionType"] == "hotkey"),
            "{state}"
        );
        assert!(
            actions
                .iter()
                .any(|a| a["actionType"] == "click" && a["payload"]["button"] == "right"),
            "{state}"
        );
        assert!(
            actions.iter().any(|a| a["actionType"] == "appSwitch"
                && a["sourceWindowId"] != initial.window.to_string()),
            "{state}"
        );
        assert!(
            actions.iter().any(|a| a["actionType"] == "keyPress"
                && a["payload"]["keyName"] == "Enter"
                && a["sourceWindowId"] != initial.window.to_string()),
            "{state}"
        );
        assert!(
            state["diagnostics"]["context_unavailable"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert!(
            actions.iter().any(|a| a["actionType"] == "secureField"),
            "{state}"
        );
        assert!(!state.to_string().contains("DO_NOT_RETAIN_PASSWORD"));
        assert!(!state["active"].as_bool().unwrap());
    }

    #[test]
    #[ignore = "interactive WGC/OCR fixture; run products/cua-win/verify-teaching.ps1"]
    fn native_teaching_screenshot_ocr_and_sensitive_gate() {
        use cua_driver_core::perception_client::{PerceptionClient, PerceptionWorkerConfig};
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        let root = std::path::PathBuf::from(
            std::env::var_os("CUA_TEACHING_TEST_BUNDLE").expect("test requires the built bundle"),
        );
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(root.join("cua-win-package.json")).unwrap())
                .unwrap();
        let version = manifest["product_version"].as_str().unwrap();
        let mut config = PerceptionWorkerConfig::installed_with_identity(
            root.join("perception/cua-perception.exe"),
            "cua-perception",
            version,
        );
        config.args = vec![
            "--manifest".into(),
            root.join("perception/model-manifest.json")
                .to_string_lossy()
                .into_owned(),
            "--onnx-runtime-library".into(),
            root.join("perception/runtime/onnxruntime.dll")
                .to_string_lossy()
                .into_owned(),
            "--extension-id".into(),
            "cua-perception".into(),
            "--extension-version".into(),
            version.into(),
        ];
        let output_parent = tempfile::tempdir().unwrap();
        let output = Arc::new(teaching::SessionOutput::create(output_parent.path()).unwrap());
        let log = Arc::new(evidence::EvidenceLog::new(
            PerceptionClientHandle::fixed(
                PerceptionClient::new(config.with_bounded_reuse(Default::default())).unwrap(),
            ),
            output.clone(),
        ));
        let fixture = Fixture::new();
        fixture.command(1);
        thread::sleep(Duration::from_millis(150));
        let mut label_rect = RECT::default();
        unsafe {
            GetWindowRect(HWND(fixture.label as *mut _), &mut label_rect).unwrap();
        }
        let raw = identity(Kind::Click(
            (label_rect.left + label_rect.right) / 2,
            (label_rect.top + label_rect.bottom) / 2,
            "left",
        ))
        .unwrap();
        assert_eq!(raw.pid, std::process::id());
        let proof = context(raw);
        assert!(proof.safe);
        assert!(
            proof.value.is_none(),
            "fixture must exercise missing ValuePattern"
        );
        log.request(raw, proof.clone(), "text_without_value_pattern");
        let deadline = Instant::now() + Duration::from_secs(15);
        let captured = loop {
            let mut result = ToolResult::text("").with_structured(json!({}));
            log.append_page(&mut result, 0, 8);
            if result.structured_content.as_ref().unwrap()["evidence"]["pending"] == 0 {
                break result;
            }
            assert!(Instant::now() < deadline, "evidence deadline");
            thread::sleep(Duration::from_millis(50));
        };
        let data = captured.structured_content.as_ref().unwrap();
        let entry = &data["evidence"]["entries"][0];
        assert_eq!(entry["status"], "captured", "{data}");
        assert_eq!(entry["source_window"]["id"], raw.window);
        assert!(entry["capture_started_at_epoch_ms"].as_u64().unwrap() >= raw.at);
        assert_eq!(entry["ocr"]["status"], "available", "{data}");
        let texts = entry["ocr"]["regions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["text"].as_str())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            texts.contains("你好") && texts.contains("世界"),
            "OCR: {texts}; {data}"
        );
        let png = std::fs::read(output.root.join(entry["image"].as_str().unwrap())).unwrap();
        let image = image::load_from_memory(&png).unwrap();
        assert_eq!(
            image.width() as u64,
            entry["geometry"]["width"].as_u64().unwrap()
        );
        assert_eq!(
            image.height() as u64,
            entry["geometry"]["height"].as_u64().unwrap()
        );
        if let Some(output) = std::env::var_os("CUA_TEACHING_TEST_OUTPUT") {
            let output = std::path::PathBuf::from(output);
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(output.join("evidence-crop.png"), &png).unwrap();
            std::fs::write(
                output.join("evidence-native.json"),
                serde_json::to_vec_pretty(data).unwrap(),
            )
            .unwrap();
        }
        let wait_for_evidence = || {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let mut result = ToolResult::text("").with_structured(json!({}));
                log.append_page(&mut result, 0, 8);
                if result.structured_content.as_ref().unwrap()["evidence"]["pending"] == 0 {
                    return result.structured_content.unwrap();
                }
                assert!(Instant::now() < deadline, "negative evidence deadline");
                thread::sleep(Duration::from_millis(20));
            }
        };
        log.request(raw, proof.clone(), "click");
        let click = wait_for_evidence();
        assert_eq!(click["evidence"]["entries"][1]["status"], "captured");
        assert_eq!(
            click["evidence"]["entries"][1]["ocr"]["status"],
            "not_needed"
        );
        fixture.command(4);
        log.request(
            Raw {
                window: identity(Kind::Focus).unwrap().window,
                ..raw
            },
            proof.clone(),
            "target_changed",
        );
        let changed = wait_for_evidence();
        assert_eq!(
            changed["evidence"]["entries"][2]["reason"],
            "privacy_or_context_changed"
        );
        fixture.command(2);
        thread::sleep(Duration::from_millis(100));
        let raw = identity(Kind::Focus).unwrap();
        let sensitive = context(raw);
        assert!(sensitive.sensitive);
        // Exercise the capture lane itself, not just the caller-side gate.
        log.request(raw, proof.clone(), "password_probe");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut result = ToolResult::text("").with_structured(json!({}));
            log.append_page(&mut result, 0, 8);
            if result.structured_content.as_ref().unwrap()["evidence"]["pending"] == 0 {
                break;
            }
            assert!(Instant::now() < deadline, "password proof deadline");
            thread::sleep(Duration::from_millis(20));
        }
        fixture.command(1);
        thread::sleep(Duration::from_millis(100));
        let raw = identity(Kind::Click(
            (label_rect.left + label_rect.right) / 2,
            (label_rect.top + label_rect.bottom) / 2,
            "left",
        ))
        .unwrap();
        let mut wrong_identity = proof.clone();
        wrong_identity.runtime_id.as_mut().unwrap().push(-1);
        log.request(raw, wrong_identity, "wrong_runtime_id");
        assert_eq!(
            wait_for_evidence()["evidence"]["entries"][4]["reason"],
            "privacy_or_context_changed"
        );
        let mut wrong_rect = proof;
        wrong_rect.rect[0] += 1;
        log.request(raw, wrong_rect, "geometry_changed");
        assert_eq!(
            wait_for_evidence()["evidence"]["entries"][5]["reason"],
            "privacy_or_context_changed"
        );
        log.finish(Duration::from_secs(2));
        let mut final_result = ToolResult::text("").with_structured(json!({}));
        log.append_page(&mut final_result, 0, 8);
        let data = final_result.structured_content.unwrap();
        assert_eq!(
            data["evidence"]["entries"][3]["status"], "skipped",
            "{data}"
        );
        assert!(data["evidence"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .skip(2)
            .all(|entry| entry.get("image").is_none()));
        assert_eq!(
            data["evidence"]["entries"][3]["reason"],
            "privacy_or_context_changed"
        );
        assert!(!data.to_string().contains("DO_NOT_RETAIN_PASSWORD"));
    }
}
