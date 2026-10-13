# Shared product refactor baseline

Frozen before P1 on 2026-10-13:

- Driver release: `cua-driver-rs-v0.34.1`, `0c69d9a2c8abbb5051a2df4a23f0fa166d5385dc`.
- Perception release: `cua-perception-v0.2.1`, `6aa37d0751d094bfe8fdc2df331ed9eae934c832`.
- Product implementation: `3d6c4147b` (dev.11).
- Independent cursor theme: `fa91655c1bfea77869bc906cebecb077c2f81aa8`.

The baseline remains recoverable through these commits. The existing dev.11
runtime and installer under `products/cua-win/out/` are retained as local
comparison artifacts; caches and outputs are not source files.

## Behavior to preserve

`start_teaching`, `get_teaching_state`, `stop_teaching` retain their arguments,
owner/transport binding, explicit CLI session requirement, runtime-local
automation exclusion, 600-second bound and cancellation semantics. Outputs
remain session.json, immutable actions, relative images/evidence and complete.json.
Action completeness is independent of optional evidence completeness.

Regression owners: shared Timeline/output tests; Windows teaching tests for IME
composition/cancellation, paste lookahead, settling text before switching,
HWND-less identity and double-click; native WGC/privacy/OCR tests; package and
persistent CLI/MCP tests. Baseline evidence: 281 Windows unit tests, four native
teaching tests, twelve package negatives; 102 separate cursor tests. Physical
keyboard and Windows 1903 acceptance are not implied by those counts.

## Downstream integration points

- Contract `lib.rs` and `teaching.rs`: tool declarations.
- Core `lib.rs`, `teaching`, `tool.rs`, `authorization.rs`: session/output,
  admission and existing permission adapters.
- Platform `lib.rs` and teaching adapter: native listening/context/capture.
- Driver `main.rs`, `product.rs`, Cargo feature: product initialization and
  registration in the existing host lifecycle.
- Driver `bundle.rs`, `extension_manager.rs`, `version_check.rs`: product
  identity and fixed bundled perception; upstream update paths stay separate.
- `cursor-overlay/motion_defaults.rs`, `pip-preview/lib.rs`: product config path.
- `products/cua`: pinned inputs, package assembly, platform builds and checks.
- Shared cursor theme source and compiled assets: independent theme commit.

## Acceptance boundary

Windows is the current execution host. No Mac is available; the user explicitly
approved completing the locally verifiable work first. Sharedization does not
constitute macOS acceptance. macOS build, TCC permissions, event capture,
screenshots, cleanup and installation each require native evidence.
