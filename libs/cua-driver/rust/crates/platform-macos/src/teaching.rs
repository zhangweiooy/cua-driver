//! macOS backend for the shared teaching service.
//! Event-tap ownership/teardown and off-callback AX reads follow the Screenpipe
//! implementation inspected at 46e35e0fd55039aa4218866d62060b07f7c3f5e8.
//! See products/cua/macos/README.md for native acceptance still required.
use super::teaching_input::{enqueue, key, verify_input};
use crate::ax::bindings::*;
use core_foundation::{
    base::{CFEqual, CFHash, CFRelease, CFTypeRef, TCFType},
    mach_port::CFMachPortInvalidate,
    runloop::{
        kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopRunResult, CFRunLoopSource,
        CFRunLoopSourceInvalidate,
    },
};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    EventField,
};
use cua_driver_core::teaching::{
    now, Backend, Captured, Context, Counters, Key, Kind, Listener, Raw,
};
use foreign_types::ForeignType;
use objc2::{msg_send, rc::autoreleasepool, ClassType};
use objc2_app_kit::NSEvent;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> i32;
    fn CGPreflightListenEventAccess() -> bool;
}

// AX remote objects are reference-counted CF objects. All AX queries below are
// serialized; no AppKit view object is transported between threads.
struct Element(AXUIElementRef);
unsafe impl Send for Element {}
unsafe impl Sync for Element {}
impl Drop for Element {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0 as CFTypeRef);
        }
    }
}
struct Proof {
    element: Element,
    rect: [f64; 4],
}
static AX_LOCK: Mutex<()> = Mutex::new(());

fn ax_timeout() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| unsafe {
        let system = AXUIElementCreateSystemWide();
        if !system.is_null() {
            AXUIElementSetMessagingTimeout(system, 0.2);
            CFRelease(system as CFTypeRef);
        }
    });
}

// Read the binding already in the event, never the current foreground window.
// A global event without a window number remains unbound and is counted as loss.
fn event_window(kind: CGEventType, event: &CGEvent) -> u64 {
    let number = match kind {
        CGEventType::LeftMouseDown
        | CGEventType::RightMouseDown
        | CGEventType::OtherMouseDown
        | CGEventType::ScrollWheel => event.get_integer_value_field(
            EventField::MOUSE_EVENT_WINDOW_UNDER_MOUSE_POINTER_THAT_CAN_HANDLE_THIS_EVENT,
        ),
        CGEventType::KeyDown | CGEventType::FlagsChanged => autoreleasepool(|_| unsafe {
            // objc2-app-kit 0.2 omits this CGEvent factory binding. The object is
            // autoreleased; its scalar windowNumber is copied before pool drain.
            let native: *mut NSEvent = msg_send![NSEvent::class(), eventWithCGEvent: event.as_ptr().cast::<std::ffi::c_void>()];
            native
                .as_ref()
                .map_or(0, |event| event.windowNumber() as i64)
        }),
        _ => 0,
    };
    u32::try_from(number).map(u64::from).unwrap_or(0)
}

fn identity(kind: Kind) -> Option<Raw> {
    let windows = crate::windows::visible_windows();
    let window = match kind {
        Kind::Click(x, y, _) | Kind::Scroll(x, y, _, _) => windows.iter().find(|w| {
            let b = &w.bounds;
            f64::from(x) >= b.x
                && f64::from(y) >= b.y
                && f64::from(x) < b.x + b.width
                && f64::from(y) < b.y + b.height
        }),
        _ => {
            let pid = crate::apps::frontmost_pid()?;
            windows.iter().find(|w| w.pid == pid)
        }
    }?;
    Some(Raw {
        window: window.window_id.into(),
        pid: window.pid as u32,
        focus: window.window_id.into(),
        at: now(),
        kind,
    })
}

unsafe fn resolve(raw: Raw) -> Option<Element> {
    let current = identity(raw.kind)?;
    if (current.window, current.pid) != (raw.window, raw.pid) {
        return None;
    }
    let element = Element(match raw.kind {
        Kind::Click(x, y, _) => element_at_screen_position(raw.pid as i32, x.into(), y.into())?,
        _ => focused_element_of_pid(raw.pid as i32)?,
    });
    let mut pid = 0;
    if AXUIElementGetPid(element.0, &mut pid) != 0 || pid as u32 != raw.pid {
        return None;
    }
    let window = copy_element_attr(element.0, "AXWindow").map(Element);
    let id = window
        .as_ref()
        .and_then(|w| ax_get_window_id(w.0))
        .or_else(|| ax_get_window_id(element.0))?;
    (u64::from(id) == raw.window).then_some(element)
}

unsafe fn password(element: &Element) -> Option<bool> {
    let role = copy_string_attr(element.0, "AXRole")?;
    let subrole = copy_string_attr(element.0, "AXSubrole");
    if role == "AXSecureTextField" || subrole.as_deref() == Some("AXSecureTextField") {
        return Some(true);
    }
    if matches!(role.as_str(), "AXTextField" | "AXTextArea" | "AXComboBox") && subrole.is_none() {
        return None;
    }
    Some(false)
}

fn read_context(raw: Raw) -> Context {
    if !matches!(raw.kind, Kind::Click(..)) && now().saturating_sub(raw.at) > 250 {
        return Context::default();
    }
    ax_timeout();
    let _guard = AX_LOCK.lock().unwrap();
    unsafe {
        let Some(element) = resolve(raw) else {
            return Context::default();
        };
        match password(&element) {
            Some(true) => {
                return Context {
                    sensitive: true,
                    ..Default::default()
                }
            }
            None => return Context::default(),
            Some(false) => {}
        }
        let Some(rect) = element_screen_rect(element.0) else {
            return Context::default();
        };
        let id = format!(
            "{}:{}:{}",
            raw.pid,
            raw.window,
            CFHash(element.0 as CFTypeRef)
        );
        let role = copy_string_attr(element.0, "AXRole");
        let title = copy_string_attr(element.0, "AXTitle");
        let identifier = copy_string_attr(element.0, "AXIdentifier");
        let value = copy_string_attr(element.0, "AXValue").filter(|s| s.len() <= 16384);
        // Only the provider's value is observed. Raw Unicode keystrokes never
        // become typeText; IME behavior requires native provider acceptance.
        let Some(after) = resolve(raw) else {
            return Context::default();
        };
        if CFEqual(element.0 as CFTypeRef, after.0 as CFTypeRef) == 0 {
            return Context::default();
        }
        let element_json = json!({"role":role,"title":title,"identifier":identifier});
        let window = crate::windows::visible_windows()
            .into_iter()
            .find(|w| u64::from(w.window_id) == raw.window && w.pid as u32 == raw.pid);
        let app = window.as_ref().map(|w| w.app_name.clone());
        Context {
            safe: true,
            sensitive: false,
            focus: id.clone(),
            click_target: id,
            element: element_json.clone(),
            value,
            metadata: json!({"app":app.clone().unwrap_or_else(||format!("pid:{}",raw.pid)),"processName":app,"windowTitle":window.map(|w|w.title),"url":null,"element":element_json}),
            proof: Some(Arc::new(Proof { element, rect })),
        }
    }
}

fn context(raw: Raw) -> Context {
    static WORKER: OnceLock<SyncSender<(Raw, SyncSender<Context>)>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<(Raw, SyncSender<Context>)>(1);
        thread::Builder::new()
            .name("cua-teaching-ax".into())
            .spawn(move || {
                for (raw, reply) in rx {
                    let _ = reply.try_send(read_context(raw));
                }
            })
            .expect("create teaching AX worker");
        tx
    });
    let (tx, rx) = mpsc::sync_channel(1);
    if worker.try_send((raw, tx)).is_err() {
        return Context::default();
    }
    rx.recv_timeout(Duration::from_millis(750))
        .unwrap_or_default()
}

fn prove(raw: Raw, proof: &Proof) -> Result<(), &'static str> {
    ax_timeout();
    let _guard = AX_LOCK.lock().unwrap();
    unsafe {
        let element = resolve(raw).ok_or("privacy_or_context_changed")?;
        if password(&element) != Some(false)
            || CFEqual(element.0 as CFTypeRef, proof.element.0 as CFTypeRef) == 0
            || element_screen_rect(element.0) != Some(proof.rect)
        {
            return Err("privacy_or_context_changed");
        }
    }
    Ok(())
}

struct TapGuard<'a> {
    tap: CGEventTap<'a>,
    source: CFRunLoopSource,
    runloop: CFRunLoop,
}
impl Drop for TapGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            self.runloop
                .remove_source(&self.source, kCFRunLoopDefaultMode);
            CFRunLoopSourceInvalidate(self.source.as_concrete_TypeRef());
            CFMachPortInvalidate(self.tap.mach_port.as_concrete_TypeRef());
        }
    }
}
struct Hooks {
    failed: Arc<AtomicBool>,
    accepting: Arc<AtomicBool>,
    done: mpsc::Receiver<()>,
}
impl Listener for Hooks {
    fn failure_reason(&self) -> Option<&'static str> {
        self.failed
            .load(Ordering::Acquire)
            .then_some("native_listener_stopped")
    }
    fn stop(&self) {
        self.accepting.store(false, Ordering::Release);
        let _ = self.done.recv_timeout(Duration::from_millis(500));
    }
}
impl Drop for Hooks {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct MacosBackend;
impl Backend for MacosBackend {
    fn check_available(&self) -> Result<(), String> {
        if !crate::permissions::status::accessibility_granted() {
            return Err("teaching_accessibility_permission_required".into());
        }
        if !unsafe { CGPreflightListenEventAccess() } {
            return Err("teaching_input_monitoring_permission_required".into());
        }
        if crate::apps::frontmost_pid().is_none() {
            return Err("teaching_desktop_unavailable".into());
        }
        Ok(())
    }
    fn start(
        &self,
        tx: SyncSender<Raw>,
        accepting: Arc<AtomicBool>,
        counters: Arc<Counters>,
        excluded: Vec<u32>,
    ) -> Result<Box<dyn Listener>, String> {
        self.check_available()?;
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let (done_tx, done) = mpsc::sync_channel(1);
        let failed = Arc::new(AtomicBool::new(false));
        let failure = failed.clone();
        let active = accepting.clone();
        thread::Builder::new()
            .name("cua-teaching-event-tap".into())
            .spawn(move || {
                let stopped = active.clone();
                let losses = counters.clone();
                let tap_failed = failure.clone();
                let tap = CGEventTap::new(
                    // Observe after the system annotates the receiving app/window.
                    CGEventTapLocation::AnnotatedSession,
                    CGEventTapPlacement::TailAppendEventTap,
                    CGEventTapOptions::ListenOnly,
                    vec![
                        CGEventType::KeyDown,
                        CGEventType::FlagsChanged,
                        CGEventType::LeftMouseDown,
                        CGEventType::RightMouseDown,
                        CGEventType::OtherMouseDown,
                        CGEventType::ScrollWheel,
                    ],
                    move |_, kind, event| {
                        if !stopped.load(Ordering::Acquire) {
                            return None;
                        }
                        // A disabled tap means events may have been lost. Stop explicitly;
                        // do not silently resume and report a complete demonstration.
                        if matches!(
                            kind,
                            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
                        ) {
                            losses.dropped.fetch_add(1, Ordering::Relaxed);
                            tap_failed.store(true, Ordering::Release);
                            stopped.store(false, Ordering::Release);
                            return None;
                        }
                        let flags = event.get_flags().bits();
                        let mods = (((flags >> 17) & 1)
                            | (((flags >> 18) & 1) << 1)
                            | (((flags >> 19) & 1) << 2)
                            | (((flags >> 20) & 1) << 3)) as u8;
                        let point = event.location();
                        let x = point.x.round() as i32;
                        let y = point.y.round() as i32;
                        // Read only fields already in the event. Native window/AX
                        // queries run after dequeue, never on the tap's deadline.
                        let pid = u32::try_from(
                            event.get_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID),
                        )
                        .unwrap_or(0);
                        if pid != 0 && excluded.contains(&pid) {
                            return None;
                        }
                        let at = now();
                        let window = event_window(kind, event);
                        let send = |kind| enqueue(&tx, &losses, kind, window, pid, at);
                        if matches!(kind, CGEventType::OtherMouseDown)
                            && event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER)
                                != 2
                        {
                            return None;
                        }
                        let normalized = match kind {
                            CGEventType::KeyDown => Kind::Key(
                                key(
                                    event.get_integer_value_field(
                                        EventField::KEYBOARD_EVENT_KEYCODE,
                                    ),
                                    mods,
                                ),
                                mods,
                            ),
                            CGEventType::FlagsChanged => Kind::Key(Key::Modifier, mods),
                            CGEventType::LeftMouseDown => Kind::Click(x, y, "left"),
                            CGEventType::RightMouseDown => Kind::Click(x, y, "right"),
                            CGEventType::OtherMouseDown => Kind::Click(x, y, "middle"),
                            CGEventType::ScrollWheel => {
                                let dy = event.get_integer_value_field(
                                    EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1,
                                ) as i32;
                                let dx = event.get_integer_value_field(
                                    EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2,
                                ) as i32;
                                // Match the retained teaching wheel convention: positive X right, Y up.
                                for (delta, horizontal) in
                                    [(dy, false), (dx.saturating_neg(), true)]
                                {
                                    if delta != 0 {
                                        send(Kind::Scroll(
                                            x,
                                            y,
                                            delta.saturating_mul(120),
                                            horizontal,
                                        ));
                                    }
                                }
                                return None;
                            }
                            _ => return None,
                        };
                        send(normalized);
                        None
                    },
                );
                let result = (|| -> Result<(), String> {
                    let tap = tap.map_err(|_| "teaching_input_monitoring_permission_required")?;
                    let source = match tap.mach_port.create_runloop_source(0) {
                        Ok(source) => source,
                        Err(_) => {
                            unsafe {
                                CFMachPortInvalidate(tap.mach_port.as_concrete_TypeRef());
                            }
                            return Err("teaching_hook_creation_failed".into());
                        }
                    };
                    let guard = TapGuard {
                        tap,
                        source,
                        runloop: CFRunLoop::get_current(),
                    };
                    unsafe {
                        guard
                            .runloop
                            .add_source(&guard.source, kCFRunLoopDefaultMode);
                    }
                    guard.tap.enable();
                    let _ = ready_tx.send(Ok(()));
                    while active.load(Ordering::Acquire) {
                        let result = CFRunLoop::run_in_mode(
                            unsafe { kCFRunLoopDefaultMode },
                            Duration::from_millis(20),
                            true,
                        );
                        if matches!(
                            result,
                            CFRunLoopRunResult::Finished | CFRunLoopRunResult::Stopped
                        ) {
                            if active.load(Ordering::Acquire) {
                                failure.store(true, Ordering::Release);
                                counters.dropped.fetch_add(1, Ordering::Relaxed);
                            }
                            active.store(false, Ordering::Release);
                            break;
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    let _ = ready_tx.send(Err(error));
                }
                let _ = done_tx.send(());
            })
            .map_err(|_| "teaching_thread_failed")?;
        match ready.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Box::new(Hooks {
                accepting,
                done,
                failed,
            })),
            Ok(Err(error)) => {
                accepting.store(false, Ordering::Release);
                Err(error)
            }
            Err(_) => {
                accepting.store(false, Ordering::Release);
                Err("teaching_hook_timeout".into())
            }
        }
    }
    fn resolve_input(&self, event: Raw) -> Option<Raw> {
        verify_input(event, now(), || identity(event.kind))
    }

    fn current(&self) -> Option<Raw> {
        identity(Kind::Focus)
    }
    fn context(&self, raw: Raw) -> Context {
        context(raw)
    }
    fn is_transient(&self, _: Raw) -> bool {
        false
    }
    fn double_click_settings(&self) -> (u64, i64) {
        let seconds = unsafe { objc2_app_kit::NSEvent::doubleClickInterval() };
        ((seconds * 1000.0).round() as u64, 4)
    }
    fn key_metadata(&self, _: Key) -> Value {
        json!({})
    }
    fn capture(
        &self,
        raw: Raw,
        proof: &(dyn std::any::Any + Send + Sync),
        queued: Instant,
        trigger: &'static str,
    ) -> Result<Captured, &'static str> {
        if !crate::permissions::status::screen_recording_granted() {
            return Err("screen_recording_permission_required");
        }
        let proof = proof.downcast_ref::<Proof>().ok_or("privacy_unknown")?;
        prove(raw, proof)?;
        if queued.elapsed() > Duration::from_millis(500) {
            return Err("capture_admission_expired");
        }
        let window = crate::windows::visible_windows()
            .into_iter()
            .find(|w| u64::from(w.window_id) == raw.window && w.pid as u32 == raw.pid)
            .ok_or("window_geometry_unavailable")?;
        let bounds = window.bounds;
        let [x, y, width, height] = proof.rect;
        if ![x, y, width, height, bounds.width, bounds.height]
            .iter()
            .all(|v| v.is_finite())
            || width <= 0.0
            || height <= 0.0
            || x < bounds.x
            || y < bounds.y
            || x + width > bounds.x + bounds.width
            || y + height > bounds.y + bounds.height
        {
            return Err("crop_geometry_unavailable");
        }
        let started = now();
        let png = crate::capture::screenshot_window_bytes(raw.window as u32)
            .map_err(|_| "target_capture_failed")?;
        let finished = now();
        let image = image::load_from_memory(&png).map_err(|_| "image_decode_failed")?;
        if u64::from(image.width()) * u64::from(image.height()) > 32_000_000 {
            return Err("window_capture_size_limit");
        }
        let sx = f64::from(image.width()) / bounds.width;
        let sy = f64::from(image.height()) / bounds.height;
        let left = ((x - bounds.x) * sx).floor() as u32;
        let top = ((y - bounds.y) * sy).floor() as u32;
        let right = ((x + width - bounds.x) * sx).ceil() as u32;
        let bottom = ((y + height - bounds.y) * sy).ceil() as u32;
        if right <= left
            || bottom <= top
            || right > image.width()
            || bottom > image.height()
            || right - left > 1600
            || bottom - top > 1000
        {
            return Err("crop_geometry_unavailable");
        }
        let after = crate::windows::visible_windows()
            .into_iter()
            .find(|w| u64::from(w.window_id) == raw.window && w.pid as u32 == raw.pid)
            .ok_or("capture_geometry_changed")?;
        if (
            after.bounds.x,
            after.bounds.y,
            after.bounds.width,
            after.bounds.height,
        ) != (bounds.x, bounds.y, bounds.width, bounds.height)
        {
            return Err("capture_geometry_changed");
        }
        prove(raw, proof)?;
        let crop = image.crop_imm(left, top, right - left, bottom - top);
        let mut encoded = std::io::Cursor::new(Vec::new());
        crop.write_to(&mut encoded, image::ImageFormat::Png)
            .map_err(|_| "image_encode_failed")?;
        Ok(Captured {
            png: encoded.into_inner(),
            width: crop.width(),
            height: crop.height(),
            detail: json!({"status":"captured","capture_backend":"upstream_window_capture","capture_started_at_epoch_ms":started,"capture_finished_at_epoch_ms":finished,"source_event_age_ms":started.saturating_sub(raw.at),"geometry":{"coordinate_space":"screen_points","crop":[x,y,x+width,y+height],"window_frame":[bounds.x,bounds.y,bounds.x+bounds.width,bounds.y+bounds.height],"width":crop.width(),"height":crop.height()},"privacy":"target_control_verified_before_and_after","trigger":trigger}),
        })
    }
}
