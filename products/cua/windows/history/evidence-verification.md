# Teaching screenshot/OCR evidence — 2026-10-13

## Current candidate: dev.5 mutual exclusion

`0.4.0-dev.5` replaces per-event agent attribution with session/operation
exclusion, as requested. Injected-event filtering, automation epochs,
suppression counters and screenshot automation-epoch checks were removed.
The existing teaching lifetime holds one exclusive OS lock; modifying Driver
operations take shared locks before dispatch. This also covers another product
instance. Read-only queries and teaching/cleanup controls remain usable.

Verification: **835 core + 272 Windows unit tests passed**, followed by both
native teaching/evidence fixtures. The packaged MCP test proved same-instance
and cross-instance automation refusal before input, teaching refusal during an
already-running action, unchanged target text after refusal, successful input
after stop, and normal timeout/disconnect cleanup. Screenshot/Chinese OCR and
sensitive-context tests still pass. Format and diff checks passed.

Artifact: `out/cua-win-0.4.0-dev.5-win64-setup.exe`, 94,045,159 bytes.
SHA256: `92d8e01f059f5e9bf6164c4a94300617c1aaacd7608bc8c74321a2ba01bfcbd5`.
Source: `out/source/cua-win-0.4.0-dev.5/`. Logs: `.cache/tests/exclusion-*.log`
and `teaching-results.json`. The previous dev.4 evidence record follows;
its event-attribution checks describe the superseded implementation.

## Previous candidate: dev.4

Candidate: `cua-win 0.4.0-dev.4`, local uncommitted changes on
`codex/cua-win-product`. Windows 11 x64, Rust 1.97.1 / MSVC 14.44.

The new evidence lane is documented in TEACHING.md. It retains verified
control crops, with original event/window identity and actual capture-time
geometry. It does not retain whole desktop/window screenshots. OCR runs on
those captured pixels only, using the bundled verified Chinese worker.

Initial native evidence verification passed with the preceding dev.3 model
bundle: a real WGC capture of a no-ValuePattern control was recognized as
`你好，世界`; a password control was refused without an image or secret value.
The model files are unchanged in dev.4. The final native run below used the
dev.4 worker and runtime-contract.

## Final candidate verification

- Shared core: **834 passed**.
- Windows library: **272 passed**, three native/stateful tests ignored by the
  ordinary suite. Both teaching native tests were then explicitly run and
  passed; the unrelated Credential Manager test was not run.
- Real WGC/OCR: a 520 x 40 pixel crop of a test-owned Text control with no
  ValuePattern was returned as a PNG artifact. Chinese recognition returned
  `你好，世界` (worker confidence 0.93546). Model identity reports dev.4.
- Negative native cases: a changed foreground/source context, an active
  Driver automation guard, and a password field all refused image retention.
- Previous native action pipeline: Chinese committed UIA text, clicks/right
  click/double click, scroll, hotkey, cross-window identity and redaction passed.
- Packaged MCP: 61 tools; ownership, concurrent-instance refusal, injected and
  actual Driver input suppression, timeout and EOF cleanup passed. Evidence
  cursor reads, frozen cutoff and repeated-stop equality also passed.
- Package: 24 verified resources, nine integrity negative cases, Chinese path,
  daemon lifecycle, isolated config and persistent MCP passed.
- `cargo fmt --check` and `git diff --check` passed. Test windows and worker/MCP
  processes were cleaned up. Final post-build changes were tests/docs only;
  matching sources are included alongside the installer.

## Artifact

- `out/cua-win-0.4.0-dev.4.dist/`
- `out/cua-win-0.4.0-dev.4-win64-setup.exe`: **94,049,214 bytes**.
- SHA256: `dde6c8fac25fb24f7006a0b28114ee0d50fcdcb67dec0fe0296ad9dafd9491aa`.
- Corresponding source: `out/source/cua-win-0.4.0-dev.4/`.

Hermetic evidence tests cover immutable pagination/image indices, no image
data in structured JSON, bounded bytes/entries, preservation of verified
pixels on an OCR deadline, and rejection of late OCR after the stop cutoff.

Local evidence and logs are under `.cache/tests/`: `evidence-native.json`,
`evidence-crop.png`, `evidence-native.log`, `evidence-regression.log`, and the
packaged MCP/lifecycle reports. The retained example contains only generated
fixture text, not user application content.

Still NOT_RUN: physical keyboard/real Chinese IME (user deferred participation),
Windows 10 1903, secure-desktop transition, installer install/upgrade/uninstall,
and the full real-application OCR matrix. The native fixture and worker smoke
are not substitutes for those acceptance gates. Product UI/persistence wiring
is outside this change. No commit, push or default-backend switch was requested.
