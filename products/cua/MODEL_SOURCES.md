# Model identities

All downloads occur during the developer's prepare/build step. The installed
worker uses local files only and verifies their digests before inference.

| Role | Frozen source | License |
| --- | --- | --- |
| Icon detector | Cua Perception 0.2.1 ONNX, based on microsoft/OmniParser-v2.0 revision 6600256cb0f1b07651e3bc86166196307bad7e2d | AGPL-3.0-only |
| Text detector | Cua Perception 0.2.1 PP-OCRv5 mobile detector | Apache-2.0 |
| Chinese candidate recognizer and dictionary | PaddlePaddle/PP-OCRv5_mobile_rec_onnx revision ed152b8b495f84de93cda5709d768548a9127622 | Apache-2.0 |
| CPU inference runtime | ONNX Runtime 1.26.0, from each locked Perception target release | MIT |
| Required VC++ DLLs | Microsoft Visual Studio package Microsoft.VC.14.44.17.14.CRT.Redist.X64.base, version 14.44.35211 | Microsoft Visual Studio redistributable terms |

The recognizer and its `inference.yml` are downloaded without conversion from
the immutable URLs in `upstream.lock.json`. The original English recognizer
remains in the locked upstream archive for evaluation, but is not installed
alongside the Chinese candidate. The YAML dictionary uses Paddle's appended
space class and blank index 0. The worker checks output class count against
dictionary length before admitting the model.

This is a development candidate, not a completed OCR quality qualification.
Compare the same screenshots with the original English package and the
existing RapidOCR PP-OCRv4 Chinese baseline. Production selection still needs representative real Chinese UI cases on
both platforms, including DPI/theme variation, recognition/missed-region
reports and cold/warm latency/memory measurements.

Source: https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_rec_onnx/tree/ed152b8b495f84de93cda5709d768548a9127622

On Windows, ONNX Runtime imports MSVCP140, MSVCP140_1, VCRUNTIME140 and VCRUNTIME140_1.
These four locked DLLs are deployed beside the worker; no machine-wide VC++
installer, administrator elevation or development PATH is required by this
deployment layout. Their immutable Microsoft package and individual hashes
are recorded in the lock. App-local deployment and redistribution terms:
https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files
