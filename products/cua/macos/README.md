# macOS build and native acceptance

Target: Apple Silicon, macOS 13+, matching the pinned Driver build baseline.
The Perception arm64 release and runtime-contract hashes are recorded in the
shared lock; the Chinese model/dictionary are identical to Windows.

On a Mac with Xcode command-line tools, Rust and Python 3.12+:

```sh
python3 products/cua/macos/build.py
# After preparation, --offline prohibits fetching missing inputs.
```

The script builds both pinned components, creates `Cua.app`, and invokes
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
3. Verify Chinese IME composition/selection/cancel, Cmd+V, rapid control changes,
   virtual AX controls and double-click. In particular, confirm provider AXValue
   reflects committed text, not marked composition, before accepting IME support.
4. Verify password redaction and target-checked crops on Retina/mixed-DPI screens,
   OCR evidence, moved/resized windows, lock/sleep and resource release.
5. Install the resulting package and collect native startup/model/inference and
   process-cleanup evidence. Windows results do not satisfy these rows.

Until these pass, P3 is an unvalidated native implementation and macOS P4 is
not complete. The user explicitly deferred native acceptance for lack of a Mac.
