# Development verification — 2026-10-13

For the newer `0.4.0-dev.3` native teaching candidate, see
[teaching-verification.md](teaching-verification.md). The following record is
the preserved `dev.2` packaging/OCR baseline.

Candidate: `cua-win 0.4.0-dev.2`, branch `codex/cua-win-product`.
The changes are local and uncommitted. This is not a release acceptance claim.

## Frozen inputs

- Driver: `cua-driver-rs-v0.34.1` → `0c69d9a2c8abbb5051a2df4a23f0fa166d5385dc`.
- Perception: `cua-perception-v0.2.1` → `6aa37d0751d094bfe8fdc2df331ed9eae934c832`.
- Both tags were checked against the GitHub component releases. Both source
  exports, Windows x64 release artifacts, Cargo.lock files, and the Perception
  runtime contract have frozen SHA256 values in `upstream.lock.json`.
- Each component was built using its own release source/lockfile and Rust
  1.97.1. The Driver carries the downstream `cua-win-product` feature.
- Chinese recognizer: official PP-OCRv5 mobile ONNX and YAML at revision
  `ed152b8b495f84de93cda5709d768548a9127622`. Input `x` is float32 NCHW with
  height 48; output `fetch_name_0` has 18,385 classes. YAML contains 18,383
  symbols; appended space plus CTC blank gives the required 18,385.
- ONNX Runtime: CPU 1.26.0. PE inspection identified four VC++ runtime DLL
  dependencies; their fixed Microsoft redistribution package and individual
  digests are locked and the DLLs ship beside the worker.

## Environment and results

Host: Windows 11 Home Chinese, build 26300, x64. Inno Setup 6 compiled the
installer with minimum Windows build 18362 and a user-level install identity.

| Check | Result |
| --- | --- |
| Cargo release Driver product build | PASS |
| Cargo release Perception build from its separate release tree | PASS |
| Product unit tests | PASS: 3 |
| Complete cursor-overlay library tests | PASS: 102 |
| pip-preview library tests | PASS: no tests in that target |
| Rust formatting and Git whitespace checks | PASS |
| Package integrity and Chinese filesystem path | PASS |
| Negative package/management checks | PASS: 9 cases |
| Repeated start, status and stop in an isolated installation | PASS |
| Persistent legacy MCP connection | PASS: initialization, 58 tools, repeated reads |
| Read-only Windows discovery | PASS |
| Configuration persisted only to the test-owned state path | PASS |
| MCP EOF shutdown | PASS |
| Real native ONNX inference through Driver's contained worker | PASS |
| Inno setup.exe compilation | PASS |
| Actual install/upgrade/uninstall on a clean OS | NOT RUN |
| Input delivery, WGC target capture and browser GUI acceptance | NOT RUN in this increment |
| Human teaching | NOT IMPLEMENTED in this increment |
| Windows 10 1903 | NOT RUN; user deferred provisioning and requested continued development |
| macOS/Linux native regression matrix | NOT RUN |

Negative cases cover missing and corrupt worker files, wrong protocol identity,
a model omitted from the package binding, path traversal, a missing required
CRT binding, upstream self-update, extension removal, and autostart creation.
The final worker and Driver processes were not left running after tests.

The normal upstream extension catalog and signature verification remain
unchanged. Product builds select a separate package resolver. Protocol 1
requires `cua-perception` as the wire extension id, so it is retained while
the extension version explicitly reports the downstream product version.
This avoids weakening the upstream parser or fabricating a publisher signature.

The initial native build needed Spectre-mitigated CRT libraries. A Microsoft
14.44 package was verified and unpacked into the ignored local build cache,
with process-local compiler environment setup. No system Visual Studio
installation or global PATH was modified. Future build machines must install
the documented MSVC Spectre prerequisite. The local helper is
`.cache/import-vcvars.ps1`; build logs are under `.cache/`.

## OCR smoke comparison

Frozen input: `tests/ocr-chinese-smoke.png`, SHA256
`cc28f0da6a8ce6377ad2b366414b7038fc84fe5ba27da1f6fa11806fc4349d94`.
It is a synthetic image with eight lines and 48 reference Unicode characters,
including spaces and punctuation. It is **not** a real-application dataset.

| Recognizer | Edit distance | CER | One end-to-end call |
| --- | ---: | ---: | ---: |
| Upstream English PP-OCRv5 | 31 / 48 | 64.58% | 3.290 s |
| Chinese PP-OCRv5 candidate | 1 / 48 | 2.08% | 2.724 s |
| Existing Chinese PP-OCRv4 recognizer | 1 / 48 | 2.08% | 2.986 s |

All three use the same upstream v5 text detector and native worker. The v4
row changes recognition only; it is not the entire old RapidOCR pipeline.
Both Chinese variants omit the first character of `二等座` on this image.
That common failure remains to be investigated. The timings are single-call
observations, not P50/P95 or performance guarantees. This evidence establishes
model loading, dictionary compatibility and a useful Chinese candidate; it
does not select a production winner. The 100-region/five-application dataset,
real DPI/theme coverage and resource measurements remain required.

## Artifacts and continuation

- Runtime directory: `out/cua-win-0.4.0-dev.2.dist`, 155,326,160 bytes
  (about 148.1 MiB), including 23 hash-verified resources.
- Installer: `out/cua-win-0.4.0-dev.2-win64-setup.exe`, 93,922,834 bytes
  (89.57 MiB).
- Installer SHA256: `b619d91f4ce1a92450d7709407be7799882d3b23e36e2ae9ea284113b0800a1b`.
- Separate source: `out/source/cua-win-0.4.0-dev.2/`.
- Machine-readable local evidence: `.cache/tests/package-results.json` and
  `.cache/ocr-comparison-final.json`.

Use an empty test installation directory. The development installer retains
the old AppId but refuses a detected Python installation until controlled
legacy cleanup and upgrade acceptance are implemented. The existing Oryx
backend has not been switched.

Next implementation work is the shared teaching contract/coalescer and
Windows event adapter, followed by sensitive-context gating, input exclusion,
stop-boundary convergence and product persistence. Screenpipe is an
architecture reference; no Screenpipe code has been copied. Complete the OCR
dataset and native platform acceptance alongside that work. The full plan,
P0 legacy artifact freeze, consumer migration, and P6 release gates remain open.
