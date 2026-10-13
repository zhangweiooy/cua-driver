# Downstream upgrade checklist

Keep Driver and Perception pinned independently. Never infer their versions
from the repository-wide latest release. Update only explicit lock entries
after verifying component tags, commits, release asset digests and Cargo.lock.

| Integration point | Downstream purpose | Upgrade check |
| --- | --- | --- |
| `cua-driver-contract/src/teaching.rs`, `lib.rs` | Three shared tools | Names, args, both platforms, returned action/evidence semantics |
| `cua-driver-core/src/teaching/` | Sessions, timeline, evidence, output | No platform crate dependency; keep owner/exclusion/stop and text regressions |
| Core `tool.rs`, `authorization.rs`, `lib.rs` | Runtime admission and existing permission adapters | Tool normalization, read-only cleanup, output directory authorization |
| `platform-windows/src/teaching.rs`, `lib.rs` | Hooks/UIA/WGC | IME, paste, focus boundary, HWND-less and double-click native fixtures |
| `platform-macos/src/teaching.rs`, `lib.rs` | Event tap/AX/window capture | Native build, TCC denial/revocation, IME/marked text, Retina crops, tap/CF cleanup |
| Driver `product.rs`, `build.rs`, Cargo feature | Shared target layout and fixed client | Compile both with and without `cua-product`; target triples and resource contract |
| Driver `main.rs` | Thin shared service registration; native app lifecycle | Preserve macOS main thread/permission helpers, Windows CLI/MCP |
| Driver `bundle.rs`, `extension_manager.rs`, `version_check.rs` | Product identity and bundled worker | Stable Windows installation/state; macOS bundle responsibility; component updates remain disabled |
| `cursor-overlay/motion_defaults.rs`, `pip-preview/lib.rs` | Product config path | Config override still respected |
| `products/cua/package.py`, lock | Shared model/resource assembly | Both target manifests, no runtime download, full package-check vs startup checks |
| Platform build wrappers | Binaries/installers/permissions | Isolated component-version-target caches, native package evidence |
| Cursor theme source/generator and compiled assets | Shared soft-indigo palette | Resolve generator/source conflicts, regenerate `.lottie` then `.cua-theme`; keep theme separate |

The shared output module retains its existing narrow Windows reparse-point
check to protect cancellation; it is filesystem safety, not teaching input or
UI orchestration. All UI/native-control identifiers and calls are in backends.
`keyCode` is optional native diagnostic metadata; portable consumers use keyName
and modifiers. Backend proof objects are not serialized into the public protocol.

On each upgrade:

1. Compare component protocol, model manifest fields, platform APIs and minimum
   OS versions. A target absent from a component release is not silently added.
2. Prepare new immutable inputs in a new component/target cache. Never copy a
   prior version's build output into the new release. Chinese weights remain
   versioned by their one source revision and digest definition.
3. Run shared and native regression owners. Do not replace native evidence with
   syntax checks, mocked tools or another OS's results. Test permission failures,
   stopping/cancellation/disconnection and no lingering hooks/workers.
4. Build final installers and verify package hashes and real offline model
   loading. macOS production signing/notarization requires a native workflow and
   validation of the signed payload; the current script emits a development pkg.
5. Keep shared core, each native backend, packaging and theme in separate commits.
   Record tested artifacts and the exact native evidence/remaining gaps.

No plugin framework, cross-process coordination, database, old API compatibility
layer or replacement updater belongs to this change.
