# Native teaching verification — 2026-10-13

Candidate: `cua-win 0.4.0-dev.3`, branch `codex/cua-win-product`, uncommitted
downstream changes on Driver `0c69d9a2c8abbb5051a2df4a23f0fa166d5385dc`.
Host: Windows 11 x64, build 26300. Rust 1.97.1 / MSVC 14.44; Inno Setup 6.

## Passed

| Boundary | Evidence |
| --- | --- |
| Shared core regression | 834 tests passed, including six new teaching contract/coalescing tests |
| Windows regression | 268 tests passed; two desktop/state-mutating tests excluded from ordinary cargo test |
| Native teaching fixture | The new ignored teaching test was explicitly run and passed; the unrelated Windows Credential Manager mutation test was not run |
| Native UIA/event processing | Two temporary Win32 windows: Chinese committed ValuePattern insertion, left double-click, right-click, scroll merging, hotkey, Enter, cross-window source attribution |
| Privacy/freshness | Native password EDIT returns sensitive without its value; password text absent from final result; an expired raw event fails closed and increments context_unavailable |
| Lifecycle over persistent MCP | Duplicate start, wrong session/owner, another process contending for the desktop, stable repeated stop/cutoff, one-second configured safety timer, EOF cleanup and subsequent owner acquisition |
| Automation exclusion | Native injected F24 excluded; actual Driver type_text delivered `Agent-only示教排除` to the disposable EDIT and was absent from teaching actions |
| Packaged runtime | 24 bound files verified; 61 MCP tools; nine package negative cases; Chinese filesystem path; start/start/status/stop; isolated config and stdio EOF |
| Build/checks | Native release build and setup.exe compilation; cargo fmt --check and git diff --check |

No production test injector or UIA test-mode switch was added. The native
pipeline test feeds the real internal bounded event channel while changing only
test-owned controls; it is not a claim of physical keyboard/IME acceptance.
The MCP test separately exercises the installed Windows low-level hooks with
flagged OS input. Test windows, MCP processes and hook owners were cleaned up.

During fixture development, foreground-lock and DPI assumptions in the test
were corrected: the fixture now uses the existing attached-thread activation
primitive and actual native control bounds. The production input source was
also corrected to resolve mouse targets under the pointer instead of assuming
the pre-click foreground window. The final native test includes the corrected
path. Its final additional changes were test-only; matching test source is
included in the source attachment.

## Artifact

- `out/cua-win-0.4.0-dev.3.dist/`
- `out/cua-win-0.4.0-dev.3-win64-setup.exe`: 93,972,485 bytes
- Setup SHA256: `a2d807681088c11a2e8fb9015c88e766099467c2af5cbac1f5a1528f32b0e52b`
- Corresponding local source: `out/source/cua-win-0.4.0-dev.3/`
- Local logs: `.cache/tests/core-teaching-regression.log`,
  `windows-teaching-regression.log`, `teaching-native-uia.log`,
  `teaching-build.log`, `package-results.json`, `teaching-results.json`.

## Not accepted / not run

The user elected not to participate in physical input testing this round.
Real keyboard/Chinese IME and real business-app coverage are **NOT_RUN**.
Secure-desktop transition, Windows 10 1903, other OS native regressions,
installer install/upgrade/uninstall and the full real-image OCR matrix are also
**NOT_RUN**. Existing pinned OCR/model binaries were not changed this round.

This completes the current P3 implementation/automatic verification, with the
initial P4 gates and UIA text path. Screenshot/OCR evidence, browser URLs,
the remaining P4 input-completeness work, and P5 product persistence/UI migration
are still pending. `complete` in a session result describes observed collection
loss, not completion of these product phases. The existing Python product has
not been switched, and no commit or push was performed.
