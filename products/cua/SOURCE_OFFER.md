# Cua product development bundle sources

This is a downstream development build, not an official Cua Driver release.
The installed runtime contains only native executables, model artifacts,
ONNX Runtime and notices. It does not contain Florence or a Python runtime.

The adjacent build output `source/<product-version>/` contains both pinned upstream source
archives, the original Perception release archive with its model inputs and
conversion materials, and the downstream patch and new product files.
Use a checkout containing both pinned component tags, apply `downstream.patch`
to the pinned Driver source, and copy `products/cua/` from the attachment into
the checkout. Build preparation verifies tag identity and Driver ancestry.
The Perception release archive includes its own
`notices/SOURCE_OFFER.md` and corresponding conversion materials.
If a working-tree build includes `new-files/`, overlay those not-yet-tracked
source files onto the Driver source root as well.

This local development candidate is not a published distribution. Before distributing
a product installer, publish the matching complete source attachment and
record its immutable download URL here and on the release page. Do not ship
an installer whose source attachment is absent.

Upstream source locations:

- https://github.com/trycua/cua/tree/cua-driver-rs-v0.35.2
- https://github.com/trycua/cua/tree/cua-perception-v0.2.1
- https://github.com/trycua/cua/releases/tag/cua-perception-v0.2.1

The OmniParser detector retains its AGPL-3.0-only license after ONNX
conversion. PaddleOCR and ONNX Runtime retain their accompanying notices.
Screenpipe's bounded hook-to-worker approach was adapted for teaching with
the user's confirmed authorization. See `TEACHING.md` for the exact inspected
revision and scope; this is not a claim of an upstream permissive license.
