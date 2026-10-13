# cua-win development bundle sources

This is a downstream development build, not an official Cua Driver release.
The installed runtime contains only native executables, model artifacts,
ONNX Runtime and notices. It does not contain Florence or a Python runtime.

The adjacent build output `source/cua-win-<product-version>/` contains both pinned upstream source
archives, the original Perception release archive with its model inputs and
conversion materials, and the downstream patch and new product files.
Apply `cua-win.patch` to the Driver release source; place `product.rs` under
`libs/cua-driver/rust/crates/cua-driver/src/` and the product files under
`products/cua-win/`. The Perception release archive includes its own
`notices/SOURCE_OFFER.md` and corresponding conversion materials.
Overlay the `new-files/` directory onto the Driver source root; it contains
the newly added shared and Windows teaching modules not present in git diff.

This local development candidate is not a published distribution. Before distributing
a product installer, publish the matching complete source attachment and
record its immutable download URL here and on the release page. Do not ship
an installer whose source attachment is absent.

Upstream source locations:

- https://github.com/trycua/cua/tree/cua-driver-rs-v0.34.1
- https://github.com/trycua/cua/tree/cua-perception-v0.2.1
- https://github.com/trycua/cua/releases/tag/cua-perception-v0.2.1

The OmniParser detector retains its AGPL-3.0-only license after ONNX
conversion. PaddleOCR and ONNX Runtime retain their accompanying notices.
Screenpipe's bounded hook-to-worker approach was adapted for teaching with
the user's confirmed authorization. See `TEACHING.md` for the exact inspected
revision and scope; this is not a claim of an upstream permissive license.
