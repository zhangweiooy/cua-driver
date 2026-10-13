# Local-plan convergence — 2026-10-13

## dev.11: bounded UIA proof, independent evidence and fixed bundle lifetime

Candidate: cua-win 0.4.0-dev.11, validated on `codex/cua-win-product` before the
initial product commit.

- Screenshot admission and post-capture proof now read only target window/PID,
  UIA RuntimeId, password state and rectangle. No repeated Name, ValuePattern,
  AutomationId or localized-role reads. The initial full context supplies the
  RuntimeId; missing identity skips that image. Text identity and double-click
  coalescing retain the dev.10 rules, including HWND-less controls.
- Top-level `complete` reports action collection/storage. `evidence.complete`
  independently reports optional images/OCR. A normal, action-complete session
  writes `complete.json` even with partial evidence; the marker includes
  `evidence_complete`. Action loss/write errors still prevent completion.
- Normal startup checks manifest identity, resource paths and runtime contract
  once and reuses a fixed Perception client. Full package hashing remains in
  `package-check`; upstream worker model/runtime validation remains enabled.
  Product fingerprint scanning and live replacement resolution were removed.
  Updates require a restart.

Windows 11 verification:

- 281 platform unit tests passed (5 desktop-bound tests ignored by default).
  New regression verifies returned status, persisted session and completion
  markers for missing capture, busy OCR, evidence deadline, image write failure,
  action loss and action write failure.
- All 4 explicitly selected native teaching tests passed. One initial run
  failed to activate the disposable fixture before capture; that case passed
  when rerun alone. Capture proof rejected wrong target, password, changed
  RuntimeId and changed rectangle; positive WGC/Chinese OCR and click-without-OCR
  checks passed. These source tests used the unchanged pinned dev.10 worker/model
  bundle; final dev.11 CLI parsing separately returned `你好，世界` from the saved
  fixture crop.
- Final dev.11 package integrity, 12 negative cases, Chinese path, persistent
  MCP/CLI, two-way teaching exclusion, owner disconnect, timeout, immutable stop,
  output/cancel and daemon cleanup passed. A deliberately damaged model did not
  prevent normal Driver startup/read-only MCP calls, but upstream model loading
  rejected its hash. No test runtimes remain running.
- Offline build and installer compilation passed. Rust formatting, Python
  compile and tracked-diff whitespace checks passed. No physical-keyboard or
  Windows 1903 acceptance was added in this round.

Evidence: `.cache/tests/convergence-dev11-{unit,native,capture,package,mcp,cli,build}.log`
and `convergence-dev11-parse.json`. Source attachment:
`out/source/cua-win-0.4.0-dev.11/`. Installer:
`out/cua-win-0.4.0-dev.11-win64-setup.exe`.
SHA-256: `710c39dae4fd41b11f79e74684bac9059c989d33d9f0913d91d356e893efa6ca`.
The final source differs from the compiled source only by a corrected comment;
verification records were updated after packaging and included in the initial
product commit.

## Historical dev.6 verification

Candidate: cua-win 0.4.0-dev.6, local/uncommitted `codex/cua-win-product`.
Authority: `D:/Documents/Code/cua-pywin/docs/cua-driver-secondary-development-plan.md`.

Final offline build and Inno compilation passed. Artifact:
`out/cua-win-0.4.0-dev.6-win64-setup.exe` (94,039,242 bytes).
SHA-256: `d02d02821e6a20325c0cedeb218749a05e2a0e27d15137591458c77d6cd2a15d`.
Checksums: `out/SHA256SUMS.txt`; corresponding source:
`out/source/cua-win-0.4.0-dev.6/`. Package and CLI/MCP end-to-end checks were
rerun against the final runtime directory and passed. Rust formatting and
tracked-diff whitespace checks passed. Changes remain uncommitted.

Changes: runtime-owned teaching/automation state replaces the cross-process
lock; password property replaces keyword/whole-tree classification; a fresh
session directory replaces in-memory image delivery. Actions and image evidence
have relative file references. Stop writes state/cutoffs and, when complete, a
completion marker. Cancel removes only session-created files. CLI uses the
upstream explicit named-session lifetime; no start compatibility alias remains.
Teaching and explicit visual parsing share the upstream single warm worker.
OCR is requested only when committed control text is missing. Contracts remain
in the existing crate; no separate SDK or Oryx UI/compatibility deliverable.

Automated Windows 11 verification:

- Core: 836 passed; contract: 61 passed; Windows: 271 passed, 3 desktop tests
  excluded from the default run. The two teaching desktop tests were explicitly
  enabled and passed with disposable native fixture windows.
- Native teaching: UIA committed Chinese text, cross-window ordering, password
  redaction, hooks, bounded context/queue behavior and immutable stop results.
- Native evidence: WGC crop written as readable PNG, matching geometry, Chinese
  OCR on a control without ValuePattern, click screenshots without OCR,
  context-change/password refusal and
  frozen final records. A normal fixture whose title contains "password" is
  accepted, demonstrating removal of keyword-based classification.
- MCP: discovery, independent runtimes, same-runtime exclusion, restored typing
  after stop, owner/session rejection, stable repeated stop, safety timeout,
  owner-disconnect state on disk, completion markers and cancellation preserving
  caller-owned files and earlier sessions.
- Persistent CLI: unnamed one-shot owner rejected; explicit owner survives CLI
  process exit; repeated stop and Unicode output paths work; active automation
  blocks teaching and active teaching blocks automation in the same runtime.
- Package: all 24 files verified; 9 integrity/disabled-update negative cases;
  Unicode path; native serve/status/stop; persistent MCP with 61 tools and EOF
  cleanup. No installed production process or user configuration was changed.

Logs are under `products/cua-win/.cache/tests/simplify-*.log`; fixture output is
test-only. The first native attempt failed at foreground activation before
collection; desktop diagnostics were normal and a retry passed. MCP test
startup was corrected to use upstream environment authorization options, and
the fixture explicitly restores its edit focus before checking actual typing.
These corrections do not change production authorization or input behavior.

Release gaps retained explicitly: Windows 10 1903, real physical IME/business
applications, mixed-DPI desktop matrix, old Python installation cleanup/upgrade,
uninstall, clean offline installation and resource measurements. The user has
deferred the 1903 environment and physical input participation. These are not
reported as passed by this convergence change.
