# Cua native product

Windows and macOS share the product feature `cua-product`, teaching service,
CLI/MCP tools, default bundled vision and Chinese OCR configuration. OS APIs
are confined to the platform backends. Python is a build/test dependency only;
the installed product contains native binaries, models, runtime libraries and notices.

Current development candidate: **0.4.0-dev.13**, based on Driver **0.35.2**
and independently pinned Perception **0.2.1**. Chinese model sources are unchanged.

- [Teaching contract](TEACHING.md)
- [Windows build, identity and validation](windows/README.md)
- [macOS build, permissions and native acceptance](macos/README.md)
- [Baseline and downstream integration points](BASELINE.md)
- [Upgrade checklist](UPGRADING.md)
- [Sharedization verification and remaining native gaps](SHAREDIZATION-VERIFICATION.md)
- [Model provenance](MODEL_SOURCES.md) and [corresponding source](SOURCE_OFFER.md)

`upstream.lock.json` is the authority for component release tags, commits,
asset hashes, Chinese recognizer/dictionary and target layouts. `package.py`
verifies inputs and generates both runtime manifests. Models use upstream
configuration; the inference implementation is not patched. Every installation
includes the same Chinese OCR and icon detector; there is no runtime download
or component hot replacement.

Normal startup checks identity, resource paths and runtime contracts once and
reuses a fixed Perception client. `package-check` hashes all packaged resources;
the upstream worker also verifies models and its runtime library on loading.
Update components together and restart the process.

Prepared source, payloads and build caches are isolated by component release
and target under `.cache/`. Chinese model inputs are isolated by source revision.
`out/` contains platform installers and corresponding source material. Existing
dev.11 artifacts remain locally under the old `products/cua-win/out/` path only
as baseline evidence; active source and build entrypoints live here.

Current release targets: Windows x64 and macOS arm64. The pinned Perception
release has no Intel macOS artifact. Adding another target requires explicit
locked resources and native acceptance, not a guessed filename.

Shared build tests: `python products/cua/tests/test_packaging.py`.
Package/CLI/MCP smoke: `python products/cua/tests/test_package.py <bundle-root>`.
The macOS bundle root is `Cua.app/Contents`; the Windows root is the `.dist`
directory. See platform instructions for native permissions and demonstration
checks. P1/P2 success alone does not certify either macOS implementation or release.
