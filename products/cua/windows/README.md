# Windows build and verification

Windows x64 retains `cua-win.exe`, AppId
`B99CB8B0-5DE5-4A0B-AEF9-69A6A3F64668`, installation under
`%LOCALAPPDATA%/Programs/cua-win` and state under `%LOCALAPPDATA%/cua-win/rust`.
`CUA_WIN_STATE_DIR` remains the absolute development-state override.
The internal feature is now `cua-product`; it is not an executable rename.

From an x64 MSVC development shell (Spectre CRT, Windows SDK, Rust 1.97.1,
Python 3.12+ and Inno Setup 6):

```powershell
./products/cua/windows/build.ps1 -Python python -Installer
# Prepared inputs can be reused without downloads:
./products/cua/windows/build.ps1 -Python python -Offline -Installer
```

The wrapper builds native binaries and invokes Inno Setup. Shared `package.py`
owns model replacement, resources, manifest generation and validation. Build
directories are isolated by component release and target. Existing output is
not overwritten; preserve it or choose a new product version before rebuilding.

The runtime exposes `cua-win mcp`, `serve`, `status`, `stop`, `package-check` and
native tool calls. The common [teaching contract](../TEACHING.md) applies. UIA
queries retain the 750ms bound and point-based click enrichment. Keyboard focus
and event age are checked; IME candidates do not replace the business-field
baseline. HWND-less controls retain a consistent text identity, separate from
click coalescing. Screenshot proof uses UIA RuntimeId/password/geometry only.

```powershell
python products/cua/tests/test_package.py products/cua/out/cua-win-0.4.0-dev.12.dist
./products/cua/windows/verify-teaching.ps1 `
  -Bundle products/cua/out/cua-win-0.4.0-dev.12.dist -Python python -AllowDesktop
```

Run desktop fixtures sequentially. They create disposable windows and require
an interactive desktop; default unit tests do not inject host input. The native
fixtures, historical IME and WPF reports remain under `tests/` and `history/`.
Historical paths refer to the baseline build; current scripts use this directory.

Windows 10 1903 remains the minimum package target, but has not been tested due
to the lack of a machine. Windows 11 fixtures are not evidence of 1903 support.
Real business-app/IME, mixed-DPI, clean-machine offline and resource measurements
remain separate acceptance tasks. Per the user's development-stage decision,
old-version migration/upgrade/uninstall compatibility is not a current gate.
