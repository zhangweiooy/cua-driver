//! Explicit Windows human demonstrations. Hook callbacks only snapshot native
//! identity/input and try_send into a bounded queue. Inspired by Screenpipe's
//! windows.rs at b23efb5d876c04865e451155589783ad7911de4c; see product notices.
use cua_driver_core::{
    teaching::{self, now, Backend, Captured, Context, Counters, Key, Kind, Listener, Raw},
    tool::ToolRegistry,
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
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

use cua_driver_core::perception_client::PerceptionClientHandle;
use cua_driver_core::teaching::evidence;

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
        enqueue(Kind::Key(semantic_key(key.vkCode), mods));
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
        proof: runtime_id(&element).ok().map(|runtime_id| {
            Arc::new(WindowsProof {
                runtime_id,
                rect: [rect.left, rect.top, rect.right, rect.bottom],
            }) as Arc<dyn std::any::Any + Send + Sync>
        }),
        element: json!({"role":element.CurrentLocalizedControlType()?.to_string(),"title":name,"identifier":identifier}),
        value,
        metadata: Value::Null,
    })
}
fn raw_context(raw: Raw) -> Context {
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

use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
fn same_rect(a: RECT, b: RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}
unsafe fn frame_rect(window: u64) -> Result<RECT, &'static str> {
    let mut rect = RECT::default();
    DwmGetWindowAttribute(
        HWND(window as *mut _),
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut rect as *mut _ as *mut _,
        std::mem::size_of::<RECT>() as u32,
    )
    .map_err(|_| "window_geometry_unavailable")?;
    Ok(rect)
}
unsafe fn prove_crop(
    automation: &IUIAutomation,
    request: &CaptureRequest,
) -> Result<(), &'static str> {
    let raw = request.raw;
    if !context_target_matches(raw) {
        return Err("privacy_or_context_changed");
    }
    let matches = (|| -> windows::core::Result<bool> {
        let element = match raw.kind {
            Kind::Click(x, y, _) => automation.ElementFromPoint(POINT { x, y })?,
            _ => automation.GetFocusedElement()?,
        };
        if element.CurrentProcessId()? as u32 != raw.pid
            || element.CurrentIsPassword()?.as_bool()
            || runtime_id(&element)? != request.proof.runtime_id
        {
            return Ok(false);
        }
        let rect = element.CurrentBoundingRectangle()?;
        Ok([rect.left, rect.top, rect.right, rect.bottom] == request.proof.rect)
    })()
    .map_err(|_| "privacy_unknown")?;
    if !matches || !context_target_matches(raw) {
        return Err("privacy_or_context_changed");
    }
    Ok(())
}

// Copy the provider's opaque identity; never read names or text during crop proof.
unsafe fn runtime_id(element: &IUIAutomationElement) -> windows::core::Result<Vec<i32>> {
    use windows::Win32::System::Ole::{
        SafeArrayDestroy, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
    };
    let array = element.GetRuntimeId()?;
    let result = (|| {
        let lower = SafeArrayGetLBound(array, 1)?;
        let upper = SafeArrayGetUBound(array, 1)?;
        if upper < lower {
            return Err(windows::core::Error::from(E_FAIL));
        }
        (lower..=upper)
            .map(|index| {
                let mut part = 0i32;
                SafeArrayGetElement(array, &index, &mut part as *mut _ as *mut _)?;
                Ok(part)
            })
            .collect()
    })();
    let _ = SafeArrayDestroy(array);
    result
}
unsafe fn capture(
    automation: &IUIAutomation,
    request: &CaptureRequest,
) -> Result<(Vec<u8>, Value, u32, u32), &'static str> {
    if !crate::diagnostics::desktop_state().input_desktop_is_default() {
        return Err("desktop_unavailable");
    }
    prove_crop(automation, request)?;
    if request.queued.elapsed() > Duration::from_millis(500) {
        return Err("capture_admission_expired");
    }
    let frame = frame_rect(request.raw.window)?;
    if (frame.right as i64 - frame.left as i64) * (frame.bottom as i64 - frame.top as i64)
        > 32_000_000
    {
        return Err("window_capture_size_limit");
    }
    let [left, top, right, bottom] = request.proof.rect;
    let (width, height) = (right - left, bottom - top);
    if width <= 0
        || height <= 0
        || width > 1600
        || height > 1000
        || left < frame.left
        || top < frame.top
        || right > frame.right
        || bottom > frame.bottom
    {
        return Err("crop_geometry_unavailable");
    }
    let started = now();
    let (pixels, w, h) = crate::wgc::screenshot_window_via_wgc(request.raw.window)
        .map_err(|_| "target_capture_failed")?;
    let finished = now();
    if (w as i64, h as i64)
        != (
            (frame.right - frame.left) as i64,
            (frame.bottom - frame.top) as i64,
        )
        || !same_rect(frame, frame_rect(request.raw.window)?)
    {
        return Err("capture_geometry_changed");
    }
    prove_crop(automation, request)?;
    if !crate::diagnostics::desktop_state().input_desktop_is_default() {
        return Err("desktop_unavailable");
    }
    let mut cropped = Vec::with_capacity(width as usize * height as usize * 4);
    for row in top - frame.top..bottom - frame.top {
        let start = (row as usize * w as usize + (left - frame.left) as usize) * 4;
        cropped.extend_from_slice(&pixels[start..start + width as usize * 4]);
    }
    let png =
        cua_driver_core::image_utils::encode_bgra_to_png(&cropped, width as u32, height as u32)
            .map_err(|_| "image_encode_failed")?;
    let detail = json!({"status":"captured","capture_backend":"wgc","capture_started_at_epoch_ms":started,"capture_finished_at_epoch_ms":finished,
        "source_event_age_ms":started.saturating_sub(request.raw.at),"geometry":{"coordinate_space":"screen_physical_pixels","crop":[left,top,right,bottom],"window_frame":[frame.left,frame.top,frame.right,frame.bottom],"width":width,"height":height},
        "privacy":"target_control_verified_before_and_after","trigger":request.trigger});
    Ok((png, detail, width as u32, height as u32))
}

#[derive(Clone)]
struct WindowsProof {
    runtime_id: Vec<i32>,
    rect: [i32; 4],
}
struct CaptureRequest<'a> {
    raw: Raw,
    proof: &'a WindowsProof,
    queued: Instant,
    trigger: &'static str,
}
pub struct WindowsBackend;
impl Listener for Hooks {
    fn stop(&self) {
        Hooks::stop(self);
    }
}
impl Backend for WindowsBackend {
    fn check_available(&self) -> Result<(), String> {
        let desktop = crate::diagnostics::desktop_state();
        if desktop.input_desktop_is_default() && desktop.session_id.is_some_and(|id| id != 0) {
            Ok(())
        } else {
            Err("teaching_desktop_unavailable".into())
        }
    }
    fn start(
        &self,
        tx: SyncSender<Raw>,
        accepting: Arc<AtomicBool>,
        counters: Arc<Counters>,
        excluded: Vec<u32>,
    ) -> Result<Box<dyn Listener>, String> {
        Hooks::start(tx, accepting, counters, excluded).map(|v| Box::new(v) as Box<dyn Listener>)
    }
    fn current(&self) -> Option<Raw> {
        identity(Kind::Focus)
    }
    fn context(&self, raw: Raw) -> Context {
        context(raw)
    }
    fn is_transient(&self, raw: Raw) -> bool {
        unsafe {
            let mut class = [0u16; 128];
            let n = GetClassNameW(HWND(raw.window as *mut _), &mut class);
            let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
            class == "IME" || class == "MSCTFIME UI" || class.contains("IME.UIManager")
        }
    }
    fn capture(
        &self,
        raw: Raw,
        proof: &(dyn std::any::Any + Send + Sync),
        queued: Instant,
        trigger: &'static str,
    ) -> Result<Captured, &'static str> {
        let proof = proof
            .downcast_ref::<WindowsProof>()
            .ok_or("privacy_unknown")?;
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|_| "uia_unavailable")?;
            let result = (|| {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                        .map_err(|_| "uia_unavailable")?;
                let (png, detail, width, height) = capture(
                    &automation,
                    &CaptureRequest {
                        raw,
                        proof,
                        queued,
                        trigger,
                    },
                )?;
                Ok(Captured {
                    png,
                    detail,
                    width,
                    height,
                })
            })();
            CoUninitialize();
            result
        }
    }
    fn double_click_settings(&self) -> (u64, i64) {
        unsafe {
            (
                GetDoubleClickTime().into(),
                (GetSystemMetrics(SM_CXDOUBLECLK).max(GetSystemMetrics(SM_CYDOUBLECLK)) / 2).into(),
            )
        }
    }
    fn key_metadata(&self, key: Key) -> Value {
        let code = (0..=255).find(|code| semantic_key(*code) == key);
        code.map_or_else(|| json!({}), |code| json!({"keyCode":code}))
    }
}
fn context(raw: Raw) -> Context {
    let mut ctx = raw_context(raw);
    ctx.metadata = base(raw, &ctx);
    ctx
}
fn semantic_key(code: u32) -> Key {
    match code {
        229 => Key::Composition,
        16..=18 | 91..=92 | 160..=165 => Key::Modifier,
        48..=90 => Key::Character(char::from_u32(code).unwrap()),
        32 => Key::Character(' '),
        _ => match key_name(code).as_deref() {
            Some(name) => Key::Named(match name {
                "Backspace" => "Backspace",
                "Tab" => "Tab",
                "Enter" => "Enter",
                "Escape" => "Escape",
                "PageUp" => "PageUp",
                "PageDown" => "PageDown",
                "End" => "End",
                "Home" => "Home",
                "Left" => "Left",
                "Up" => "Up",
                "Right" => "Right",
                "Down" => "Down",
                "Delete" => "Delete",
                _ => [
                    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
                    "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23",
                    "F24",
                ][(code - 112) as usize],
            }),
            None => Key::Text,
        },
    }
}
pub fn register_into(registry: &mut ToolRegistry, perception: PerceptionClientHandle) {
    teaching::register_into(registry, perception, Arc::new(WindowsBackend));
}
#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::{
        protocol::ToolResult,
        teaching::{push, run, Live, TextCapture, Timeline},
    };

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
        for (kind, at) in [(Kind::Focus, 0), (Kind::Key(semantic_key(65), 0), 10)] {
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
        let (key, typed) = sample(Kind::Key(semantic_key(66), 0), 40, "ab");
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
                kind: Kind::Key(semantic_key(65), 0),
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
            Arc::new(WindowsBackend),
        ));
        let (t, a, r, e) = (
            timeline.clone(),
            accepting.clone(),
            reason.clone(),
            evidence.clone(),
        );
        let worker = thread::spawn(move || {
            run(
                Arc::new(WindowsBackend),
                rx,
                Box::new(hooks),
                t,
                a,
                r,
                counters,
                vec![],
                10,
                e,
            );
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
        tx.send(identity(Kind::Key(semantic_key(229), 0)).unwrap())
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        fixture.command(1);
        thread::sleep(Duration::from_millis(250));
        for kind in [
            Kind::Click(x, y, "left"),
            Kind::Click(x, y, "left"),
            Kind::Click(x, y, "right"),
            Kind::Scroll(x, y, 120, false),
            Kind::Scroll(x, y, 120, false),
            Kind::Key(semantic_key(83), 2),
        ] {
            tx.send(identity(kind).unwrap()).unwrap();
            thread::sleep(Duration::from_millis(80));
        }
        fixture.command(2);
        thread::sleep(Duration::from_millis(100));
        let password = context(identity(Kind::Focus).unwrap());
        assert!(password.sensitive && !password.safe && password.value.is_none());
        tx.send(identity(Kind::Key(semantic_key(65), 0)).unwrap())
            .unwrap();
        thread::sleep(Duration::from_millis(250));
        fixture.command(4);
        thread::sleep(Duration::from_millis(150));
        tx.send(identity(Kind::Key(semantic_key(13), 0)).unwrap())
            .unwrap();
        let mut stale = identity(Kind::Key(semantic_key(66), 0)).unwrap();
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
            Arc::new(WindowsBackend),
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
        let mut native = wrong_identity
            .proof
            .as_ref()
            .unwrap()
            .downcast_ref::<WindowsProof>()
            .unwrap()
            .clone();
        native.runtime_id.push(-1);
        wrong_identity.proof = Some(Arc::new(native));
        log.request(raw, wrong_identity, "wrong_runtime_id");
        assert_eq!(
            wait_for_evidence()["evidence"]["entries"][4]["reason"],
            "privacy_or_context_changed"
        );
        let mut wrong_rect = proof;
        let mut native = wrong_rect
            .proof
            .as_ref()
            .unwrap()
            .downcast_ref::<WindowsProof>()
            .unwrap()
            .clone();
        native.rect[0] += 1;
        wrong_rect.proof = Some(Arc::new(native));
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
