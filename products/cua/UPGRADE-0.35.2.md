# Driver 0.35.2 upgrade — 2026-10-14

Development candidate **0.4.0-dev.13** integrates the official
[Driver 0.35.2 release](https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.35.2),
commit `ad694f82aaaa12e67257e4510837fc1f6cfc9484`.
Perception remains independently pinned to **0.2.1**, ONNX Runtime to **1.26.0**;
Chinese recognition weights/dictionary and the upstream icon detector are unchanged.
The authoritative source, Cargo.lock and platform asset hashes are in
`upstream.lock.json`; both Windows and macOS release archives were downloaded
and their actual hashes verified.

## Integration

- `89988131f` merges the official release history. The 0.34.1 maintenance
  branch diverged from the 0.35 line, so upstream-only conflicts use the 0.35.2
  contents. Downstream changes were reconciled against the actual 0.34.1
  baseline, avoiding accidental retention of maintenance-branch-only modules.
- Shared teaching, native Windows/macOS adapters, fixed vision/OCR resources,
  product initialization and CLI/MCP delivery remain in their existing modules.
  Argument aliases introduced upstream run before the teaching output default;
  teaching exclusion remains at the common tool-dispatch boundary.
- The new cursor-motion crate and timing behavior follow upstream. Soft-indigo
  rendering remains in the shared overlay. Both theme artifacts were regenerated
  from source and were byte-identical to the existing checked-in artifacts.
- `6ee51ec1a` makes the new script operator opt-out honor `CUA_DRIVER_CONFIG_PATH`.
  The package regression failed on the initial merged binary and passed after
  rebuilding with this correction. No new script policy or compatibility layer
  was introduced.
- `dae9e1348` pins resources and the dev.13 product version and adds MCP coverage
  for `run_steps`, hidden `run_actions` and `run_script` during teaching.

## Upstream behavior changes to account for

This includes the full 0.35 series, not just the three 0.35.2 fixes. In particular:

- `run_steps` and the default-enabled `run_script` use the existing runtime
  permission/admission path. Both, and the hidden batch alias, are rejected
  while teaching is active. The operator can disable scripts in the product's
  config file; tools cannot override that setting through `set_config`.
- Tool schemas use the upstream Vertex-compatible representation. macOS window
  reads collapse closed menus and move additional walk diagnostics behind
  `verbose`; callers should follow the updated upstream Skills and contract.
- macOS keyboard typing treats newline/return as Return and tab as Tab on key
  delivery paths. Text containing these characters can submit or move focus.
- The old 0.34.1 `display_only` maintenance-branch API is not restored as a
  downstream compatibility layer; consumers should use the 0.35.2 contract.
- Windows receives the upstream transient-UIA/consent fixes and paced foreground
  typing. macOS receives the upstream background/Space and Electron scroll fixes.
  Their inclusion is not a claim of native macOS validation here.

## Verification

| Check | Result |
| --- | --- |
| Shared core | 970 passed; the 15 script tests passed again after the config-path correction |
| Windows unit tests | 282 passed, 5 desktop tests opt-in |
| macOS host-portable tests | 10 passed on Windows; not native macOS compilation |
| Cursor overlay / motion core | 93 / 15 passed |
| Shared packaging | 6 passed |
| Default/non-product build | `cargo check --locked -p cua-driver --bin cua-driver` passed |
| Product build | Release build with `cua-product`, version/target-isolated cache, offline final build and Inno installer passed |
| Package | 24 files verified; 12 negative cases, Chinese path, upstream model-hash rejection, persistent MCP, isolated config and daemon cleanup passed |
| Operator script opt-out | Disabled scripts absent from packaged MCP tools when set in product config |
| MCP/CLI teaching | Session ownership, runtime-local exclusion, new batch/script exclusion, timeout, disconnect, output files, repeated stop and cancellation passed |
| Native Windows teaching | Click focus, double-click, UIA text/privacy/pipeline and screenshot/OCR/sensitive gate passed |
| Fresh-directory install | Installer exit 0; installed package-check, Driver 0.35.2 version and offline Chinese OCR passed; uninstall registration/directory removed and user PATH unchanged |
| macOS resources | 22 files verified using genuine upstream binaries and the new app template; minimum macOS 13.0 and permission descriptions retained |

The native screenshot and text tests initially failed while activating their
disposable fixture (`force_foreground_attached`), before the teaching pipeline
ran. Individual reruns passed; assertions were not weakened. This fixture
activation limitation was also observed in the previous dev.12 baseline.

Installed OCR smoke recognized `你好，世界` using the bundled native worker.
This is not full OCR quality acceptance. macOS native compilation, real keyboard
window bindings, Option/IME input, TCC, callback latency and installation remain
**NOT RUN** because no Mac is available. Windows 1903, clean-machine acceptance,
real human IME and secure-desktop transitions are also not certified here.

## Artifacts and provenance

The final runtime was built from `dae9e1348`. Later verification documentation
does not change runtime sources. Artifacts remain local development outputs:

- `out/cua-win-0.4.0-dev.13.dist/`
- `out/cua-win-0.4.0-dev.13-win64-setup.exe` — 94,701,352 bytes
- Installer SHA-256: `4dd7ca89232c03bd5b61f51c2d46cffe6e3116d82f531c65c18a3259c5ea2d18`
- `out/source/0.4.0-dev.13/` — refreshed after final verification documentation

Machine-readable package/teaching results and installation logs are under
`.cache/tests/`, including `package-results.json`, `installed-dev13-results.json`,
`upgrade-0.35.2-installed-ocr.json` and `upgrade-0.35.2-macos-resources.json`.
The initial pre-correction artifacts were preserved under
`.cache/tests/upgrade-0.35.2-preliminary/`. No GitHub installer release was published.
