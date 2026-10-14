# macOS build and native acceptance

Target: Apple Silicon, macOS 13+, matching the pinned Driver build baseline.
The Perception arm64 release and runtime-contract hashes are recorded in the
shared lock; the Chinese model/dictionary are identical to Windows.

On a Mac with Xcode command-line tools, Rust and Python 3.12+:

```sh
python3 products/cua/macos/build.py
# After preparation, --offline prohibits fetching missing inputs.
```

The script builds both pinned components, retains the upstream app template's
permission usage descriptions, minimum OS declaration and resources, then
applies the product bundle identity and creates `Cua.app`. It invokes
`pkgbuild` for an **unsigned development installer**. Production signing,
notarization and signed-package integrity require their own native validation;
this script does not claim a release-ready signed distribution. Run CLI/MCP
through `Cua.app/Contents/MacOS/cua` (`mcp`, `serve`, `status`, `stop`,
`package-check`). User data defaults to `~/Library/Application Support/cua`;
`CUA_PRODUCT_STATE_DIR` selects an absolute development state directory.

Product initialization precedes ordinary command dispatch while retaining the
upstream permission-probe helper and AppKit main-thread lifecycle. The bundle
name/identifier feed upstream responsibility detection. No Windows main-loop
or permission workflow is copied into the macOS entrypoint.

## Native backend

`platform-macos/src/teaching.rs` provides a listen-only CGEventTap, native key
normalization, a bounded AX worker, AX control identity/password/value queries,
and upstream target-window capture. The shared service owns all sessions,
coalescing, OCR, evidence queues and files. Tap disablement records loss and
stops rather than silently claiming complete input. Teardown invalidates the
run-loop source and Mach port before releasing callback state.

The listen-only tap uses Apple's
[`AnnotatedSession`](https://developer.apple.com/documentation/coregraphics/cgeventtaplocation/cgannotatedsessioneventtap)
location, where session events have been annotated for delivery to an application.
The tap callback reads event fields and uses the existing 256-entry nonblocking
queue; it does not enumerate windows or query Spaces/AX. The shared consumer
verifies each queued identity once (including lookahead), retaining the original
window ID, target PID and timestamp. Mouse/scroll bindings use the event's
`mouseEventWindowUnderMousePointerThatCanHandleThisEvent` field; keyboard bindings
read [`NSEvent.windowNumber`](https://developer.apple.com/documentation/appkit/nsevent/windownumber)
from the equivalent event created by
[`eventWithCGEvent:`](https://developer.apple.com/documentation/appkit/nsevent/init%28cgevent%3A%29-4igjn).
The keyboard callback creates a temporary event object in an autorelease pool;
it does not query AX, windows, Spaces or the current foreground application.

Missing/zero bindings, changed or closed windows, PID mismatches and inputs older
than 250 ms count as dropped. The consumer never fills in a window from current
focus, including another window of the same application. No additional queue,
worker or polling loop is added. Native callback latency and global keyboard
event window-number availability still need Mac measurement. If a provider does
not supply this binding, its input is omitted with an incomplete report, rather
than attributed using current focus. Host tests cover A/B switches, closing A to
reveal B, missing bindings, queue bounds and consumer-only verification.

Option/Shift+Option printable and dead keys carry text-input intent; AX values
remain the authority for committed text. Command/Control shortcuts and Option
navigation retain shortcut semantics. The pure normalization/queue tests also
run on Windows via `cargo test --locked --release -p platform-macos --lib`.

Accessibility and Input Monitoring are required for teaching. A missing grant
returns a specific permission error; no automatic permission bypass or synthetic
input is used. Missing Screen Recording permission skips image evidence. Native
screen/point geometry is converted to pixels only inside this adapter.

Reference: Screenpipe `crates/screenpipe-a11y/src/platform/macos.rs` at
[`46e35e0fd55039aa4218866d62060b07f7c3f5e8`](https://github.com/screenpipe/screenpipe/blob/46e35e0fd55039aa4218866d62060b07f7c3f5e8/crates/screenpipe-a11y/src/platform/macos.rs),
inspected under the user's authorization. The absorbed ideas are listen-only
tap ownership, ordered teardown, off-callback AX work and bounded AX messages.
No Screenpipe capture database, clipboard polling or background recording
service is imported. The implementation uses the Driver's existing dependencies.

## Acceptance status: native validation pending

No Mac was available for this change. Source/API inspection and formatting do
not establish macOS compilation or operation. The following must run on a Mac:

1. Build the native app/worker; test fresh/denied/revoked AX, Input Monitoring
   and Screen Recording grants using the real bundle identity.
2. Run the common package/CLI/MCP suite; exercise teaching start/status/stop,
   explicit owner sessions, mutual exclusion, timeout, disconnect and cancellation.
3. Verify Chinese IME composition/selection/cancel, Cmd+V, Option+V (`√`),
   Shift+Option, Option dead-key composition/cancel, rapid control changes,
   virtual AX controls and double-click. In particular, confirm provider AXValue
   reflects committed text, not marked composition, before accepting IME support.
4. Verify password redaction and target-checked crops on Retina/mixed-DPI screens,
   OCR evidence, moved/resized windows, lock/sleep and resource release.
5. Install the resulting package and collect native startup/model/inference and
   process-cleanup evidence. Windows results do not satisfy these rows.
6. Compare tap callback latency with many windows/Spaces and delayed WindowServer
   responses; verify bounded overflow, stop/timeout cleanup and loss reporting.
   Verify event window bindings for global keys, modifier changes, clicks and
   scrolling, including rapid switches between two windows of one application
   and closing a window to expose another at the same coordinates.

Until these pass, P3 is an unvalidated native implementation and macOS P4 is
not complete. The user explicitly deferred native acceptance for lack of a Mac.
