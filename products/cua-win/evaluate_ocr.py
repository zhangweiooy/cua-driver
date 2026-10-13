"""Compare native English/v5-Chinese/v4-Chinese recognition on frozen images.

All three use the same upstream v5 detector and native worker. This isolates
recognizer compatibility; it is not a full comparison with the old RapidOCR
pipeline (which also has a v4 detector and an angle classifier).
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")


def v4_comparison(candidate, legacy, destination):
    shutil.copytree(candidate, destination)
    perception = destination / "perception"
    model_source = legacy / "ch_PP-OCRv4_rec_mobile.onnx"
    keys_source = legacy / "ppocr_keys_v1.txt"
    assert digest(model_source) == "48fc40f24f6d2a207a2b1091d3437eb3cc3eb6b676dc3ef9c37384005483683b"
    assert digest(keys_source) == "28b2362ad4ab2dc38769aa72feb535e3a9ddb3fd2a7585a05920e6393b1dc7f7"
    model = perception / "models" / model_source.name
    dictionary = perception / "models/ppocrv4-keys-with-space.txt"
    shutil.copy2(model_source, model)
    symbols = keys_source.read_text(encoding="utf-8").splitlines()
    assert len(symbols) == 6623, len(symbols)
    dictionary.write_text("\n".join(symbols + [" "]) + "\n", encoding="utf-8", newline="\n")
    manifest_path = perception / "model-manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["identity"]["name"] = "ppocrv5det-ppocrv4rec-zh-comparison"
    manifest["identity"]["source_revision"] = "detector:6600256cb0f1b07651e3bc86166196307bad7e2d;ocr-rec:RapidOCR-3.8.0;sha256:" + digest(model)
    manifest["ocr"]["recognizer"]["model"] = {"path": "models/" + model.name, "sha256": digest(model)}
    manifest["ocr"]["recognizer"]["output_name"] = "softmax_11.tmp_0"
    manifest["ocr"]["dictionary"] = {"path": "models/" + dictionary.name, "sha256": digest(dictionary)}
    manifest["ocr"]["dictionary_format"] = "plain_lines"
    write_json(manifest_path, manifest)
    contract_path = perception / "runtime-contract.json"
    contract = json.loads(contract_path.read_text(encoding="utf-8"))
    contract["modelManifest"]["sha256"] = digest(manifest_path)
    for item in contract["models"]:
        if item["role"] == "ocr-recognize":
            item.update(name=model.name, sha256=digest(model))
    contract["dictionary"].update(name=dictionary.name, sha256=digest(dictionary))
    write_json(contract_path, contract)
    package_path = destination / "cua-win-package.json"
    package = json.loads(package_path.read_text(encoding="utf-8"))
    package["files"] = {path.relative_to(destination).as_posix(): digest(path)
                        for path in sorted(destination.rglob("*"))
                        if path.is_file() and path != package_path}
    write_json(package_path, package)


def distance(expected, actual):
    previous = list(range(len(actual) + 1))
    for row, left in enumerate(expected, 1):
        current = [row]
        for col, right in enumerate(actual, 1):
            current.append(min(current[-1] + 1, previous[col] + 1,
                               previous[col - 1] + (left != right)))
        previous = current
    return previous[-1]


def parse(bundle, image, capture):
    start = time.perf_counter()
    result = subprocess.run([str(bundle / "cua-win.exe"), "perception", "parse",
                             "--image", str(image), "--capture", str(capture), "--json"],
                            capture_output=True, text=True, encoding="utf-8", timeout=90)
    elapsed = time.perf_counter() - start
    output = json.loads(result.stdout)
    assert result.returncode == 0, output
    texts = [region for region in output["regions"] if region["kind"] == "text"]
    texts.sort(key=lambda region: (region["bounds"]["y"], region["bounds"]["x"]))
    return [region["text"] for region in texts], elapsed, output["parser"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--english", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--legacy-models", required=True, type=Path)
    parser.add_argument("--samples", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    samples = json.loads(args.samples.read_text(encoding="utf-8"))
    cache = Path(__file__).resolve().parent / ".cache"
    results = {"scope": "same native v5 detector, different recognizers", "full_acceptance": False, "samples": []}
    with tempfile.TemporaryDirectory(prefix="ocr-comparison-", dir=cache) as temporary:
        temp = Path(temporary)
        comparison = temp / "v4"
        v4_comparison(args.candidate.resolve(), args.legacy_models.resolve(), comparison)
        capture = temp / "capture.json"
        write_json(capture, {"source": {"kind": "window", "pid": 123, "window_id": 456}, "snapshot_id": "offline-comparison"})
        for sample in samples:
            assert sample["kind"] in ("synthetic", "real"), sample
            image = (args.samples.parent / sample["image"]).resolve(strict=True)
            expected = "".join(sample["expected_text"])
            assert expected
            entry = {"id": sample["id"], "kind": sample["kind"], "image_sha256": digest(image), "expected_text": sample["expected_text"], "profiles": {}}
            for name, bundle in [("v5_english", args.english.resolve()), ("v5_chinese", args.candidate.resolve()), ("v4_chinese_recognizer", comparison)]:
                texts, elapsed, identity = parse(bundle, image, capture)
                edits = distance(expected, "".join(texts))
                entry["profiles"][name] = {"texts": texts, "edit_distance": edits, "reference_characters": len(expected), "cer": edits / len(expected), "total_seconds": round(elapsed, 3), "parser": identity}
            results["samples"].append(entry)
    write_json(args.output, results)
    print(json.dumps({"samples": len(results["samples"]), "full_acceptance": False,
                      "results": [{"id": x["id"], "profiles": {k: {"cer": v["cer"], "seconds": v["total_seconds"]} for k, v in x["profiles"].items()}} for x in results["samples"]]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
