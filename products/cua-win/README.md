# cua-win Windows native product

Implementation follows the local plan supplied on 2026-10-13:
`cua-pywin/docs/cua-driver-secondary-development-plan.md`.
The deliverables are a Windows setup.exe, native CLI/MCP and teaching output
files. Oryx owns its UI, Agent orchestration, LLM and long-term data handling.
Previous verification records are historical evidence, not additional scope.

## Build and runtime

Driver is pinned to `cua-driver-rs-v0.34.1`; Perception is independently pinned
to `cua-perception-v0.2.1`. `upstream.lock.json` records versions and hashes.
The product carries one Chinese PP-OCRv5 recognizer/dictionary, the upstream
OmniParser detector and CPU ONNX Runtime. Models are installed locally and are
never downloaded at runtime. No Python, PyTorch or Florence runtime is shipped.

```powershell
./products/cua-win/prepare.ps1
./products/cua-win/build.ps1 -Offline -Installer
```

Build tools: Rust 1.97.1, MSVC x64 including Spectre CRT, Windows SDK and Inno
Setup 6. End users need none of these tools. Separate source attachments include
both pinned sources, model conversion materials and downstream changes.

```powershell
./cua-win.exe package-check
./cua-win.exe mcp
./cua-win.exe serve
./cua-win.exe status
./cua-win.exe stop
```

`serve`, `status`, `stop`, `doctor` and tool calls use upstream native CLI
behavior. There is no Python CLI/MCP compatibility layer or custom start alias.
The product name, AppId B99CB8B0-5DE5-4A0B-AEF9-69A6A3F64668 and user-level
installation identity remain cua-win. Native named pipes/config remain separate
from the official Driver installation. `CUA_WIN_STATE_DIR` can point to an
absolute isolated user-data directory for development.

`package-check` verifies every packaged file's SHA-256. Normal startup checks
identity, resource paths and the runtime contract once, then reuses one fixed
Perception client. There are no per-parse package scans or live replacement;
restart after a component update. Model and ONNX Runtime hashes are checked by
the upstream worker when it loads them. Worker protocol and containment remain.
The official updater, extension install/remove and upstream
autostart setup are disabled; component updates ship together in this installer.
No replacement updater or publisher trust service is introduced.

## Teaching and verification

See [TEACHING.md](TEACHING.md) for new interfaces, runtime-local mutual exclusion,
password-only checks, relative output files, completion markers and cancellation.
CLI teaching uses a persistent serve runtime and an explicit session label.
The tool declarations live in the existing contract crate; no new SDK product
or old computer_learn translation is part of this repository.

Use `verify-teaching.ps1` for opt-in native/MCP/CLI checks and `test_package.py`
for package integrity. Chinese model comparison remains available through
`evaluate_ocr.py`; select representative real cases rather than arbitrary sample
counts. `convergence-verification.md` records this candidate's actual coverage.
`ime-verification.md` records the real Microsoft Pinyin acceptance run and the
dev.7 fixes/retests for composition-pause diagnostics and missing paste text.
Both text defects passed retesting. Dev.8 aligns click enrichment with the old
point-based/750ms behavior; the previous focus-switch click degradation also
passed retesting, including target context, screenshots and completion markers.
Dev.9 fixes confirmed text being deferred across control/window boundaries and
double-click splitting when the first click changes focus. Regression details
and final artifact checks are also recorded in `ime-verification.md`.
Dev.10 separates text-control identity from click-target identity, fixing
keyboard/click discontinuity for HWND-less UIA controls. Regression and real
WPF TextBox verification both passed; details are in the same record.
Dev.11 narrows screenshot UIA proof, separates action/evidence completeness and
removes repeated package hashing and live replacement resolution. Results are
recorded in `convergence-verification.md`.

## Remaining release acceptance

Windows 10 1903 x64 is still required and was deferred by the user for lack of
a test machine. Windows 11 fixtures do not satisfy that gate. Real IME/business
applications, mixed-DPI monitors, offline clean-machine startup and resource
measurements remain verification work. Per the user's development-stage scope,
legacy upgrade/migration/uninstall compatibility is not a current task or
acceptance gate. Use an empty test directory for this development candidate.
