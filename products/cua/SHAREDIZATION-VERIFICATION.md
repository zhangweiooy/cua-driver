# Shared product verification — 2026-10-14

This records the current development boundary, not a dual-platform release
certification. The user confirmed that no Mac is available and authorized
finishing the locally verifiable work first.

| Phase | Status and evidence |
| --- | --- |
| P0 | Baseline `fa91655c1` and downstream integration inventory preserved in `BASELINE.md`; original dev.11 artifacts retained locally |
| P1 | Shared sessions, normalized events, text/timeline, evidence/OCR and file lifecycle extracted; Windows reconnected and regression-checked |
| P2 | Neutral `products/cua`, `cua-product`, shared target initialization and standard-library Python assembly; one authoritative lock for model sources and target resources |
| P3 | macOS event tap/AX/capture backend and native lifecycle registration implemented; native compilation, TCC and real input/IME acceptance **not run** |
| P4 | Windows 11 final package, CLI/MCP, native fixtures and fresh-directory installation/OCR passed; Windows 1903, clean-machine and macOS native install/run evidence remain missing |
| P5 | Common/platform docs, source notices, integration inventory and `UPGRADING.md` provided; changes split by responsibility, no push |

## Windows and shared results

- 848 core tests passed, including text/paste/composition/control-boundary
  regressions moved into shared ownership, evidence completeness and native
  listener failure reporting.
- 270 Windows unit tests passed; five desktop-bound tests remain opt-in.
- Four selected native teaching scenarios passed against dev.12. Two initially
  failed while creating/activating their disposable fixture, before the pipeline
  ran; individual reruns passed. No refusal assertion was weakened.
- Six hermetic shared packaging tests passed. The final Windows package passed
  twelve negative cases, full integrity, Chinese-path, persistent CLI/MCP,
  runtime-local exclusion, owner disconnect, timeout, immutable stop, output
  and bounded cancellation tests.
- A real fresh-directory Inno installation passed `package-check`, version and
  native Chinese OCR (`你好，世界`). The temporary installation and its uninstall
  registration were removed; the user's PATH was unchanged. This was the current
  Windows 11 machine, not a clean OS image or old-version migration test.
- The upstream/default build without `cua-product` passed `cargo check`.
  Rust formatting, Python compilation and diff whitespace checks passed.

Final Windows runtime was built from `ac7c0d657`. Subsequent changes are macOS-only
code/build fixes, preservation of the pre-existing build ancestry check,
immutable source-export checks, documentation and formatting of the added
integration lines; no Windows runtime logic changed after that build.

Installer: `out/cua-win-0.4.0-dev.12-win64-setup.exe` (94,040,200 bytes).
SHA-256: `506f7a388bdf2158d6fdc4e28eb4b414aceb61f76da4458f1718e545aefeecbc`.
Runtime: `out/cua-win-0.4.0-dev.12.dist/`.
Corresponding source: `out/source/0.4.0-dev.12/` (refreshed after final docs).

Local logs/results under `.cache/tests/`: `shared-core-final.log`,
`shared-teaching-units.log`, `shared-native-final.log`, `shared-capture-final.log`,
`shared-input-final.log`, `shared-package-final.log`, `shared-mcp-final.log`,
`shared-cli-final.log`, `shared-product-final-build.log`,
`upstream-default-check.log`, `installed-dev12-results.json`,
`installed-dev12-parse.json` and `install-dev12.log`.

## macOS resource verification and native gap

Both real macOS release archives were fetched and verified against the locked
SHA-256 values. The shared assembler ran on Windows with the pinned upstream
macOS binaries (not a build of the new product backend). Its generated package
and upstream app template verified 22 files; the icon/text detector, Chinese
recognizer/dictionary definitions and OCR parameters exactly matched the
Windows manifest. See `.cache/tests/macos-resource-results.json`.

This validates resources and packaging rules only. No macOS binary was run,
no product macOS build or installer was produced, and no TCC, real IME, Retina
crop or event-tap cleanup claim is made. An attempted Cargo dependency fetch
also encountered the host's certificate-revocation service error; that is not
macOS compilation evidence. Native acceptance remains the checklist in
`macos/README.md`. The current pinned Perception release only supplies macOS
arm64 resources, so Intel macOS is not silently claimed as supported.

## Commit organization

- `835a8b91d`: baseline and boundaries.
- `6c33b8e43`: shared teaching service and regression ownership.
- `65f1cebd4`: Windows native adapter.
- `01a3ae08f`: shared listener-failure termination semantics.
- `6ca0b4661`: macOS native adapter, explicitly awaiting native acceptance.
- `55f783d8e`: neutral product layout, shared model assembly and packaging.
- `ac7c0d657`, `405b3df42`: macOS evidence geometry and wheel semantics.
- `7bee205ee`: inherited upstream macOS app template/resources.
- `95b30f058`, `86d0a1460`: build ancestry and immutable source exports.
- The build-provenance and documentation follow-ups remain separate from runtime
  and theme changes. The independent soft-indigo theme stays in `fa91655c1`.

Unrelated local audit scripts, skill folders and experiments were preserved.
The retired `products/cua-win/.cache/` and `out/` are locally excluded from Git;
they contain baseline artifacts, not a compatibility implementation.
