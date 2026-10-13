"""Read-only P1 package checks; no desktop input, recording or host install.

Python is a development test dependency only, never included in the runtime.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading
import time


def run(exe, *args, ok=True):
    result = subprocess.run([str(exe), *args], capture_output=True, text=True,
                            encoding="utf-8", timeout=45)
    assert (result.returncode == 0) == ok, (args, result.returncode, result.stdout, result.stderr)
    return result


def test_integrity(bundle, cache):
    assert json.loads(run(bundle / "cua-win.exe", "package-check").stdout)["success"]
    with tempfile.TemporaryDirectory(prefix="package-test-", dir=cache) as temporary:
        isolated = Path(temporary) / "中文路径"
        shutil.copytree(bundle, isolated)
        exe = isolated / "cua-win.exe"
        assert json.loads(run(exe, "package-check").stdout)["success"]
        manifest_path = isolated / "cua-win-package.json"
        original = manifest_path.read_bytes()
        manifest = json.loads(original)
        worker = isolated / "perception/cua-perception.exe"
        preserved = worker.with_suffix(".preserved")
        worker.rename(preserved)
        assert "missing package file" in run(exe, "package-check", ok=False).stderr
        assert "missing package file" in run(exe, "mcp", "--no-overlay", ok=False).stderr
        preserved.rename(worker)
        with worker.open("r+b") as stream:
            first = stream.read(1)
            stream.seek(0)
            stream.write(bytes([first[0] ^ 1]))
        assert "SHA256 mismatch" in run(exe, "package-check", ok=False).stderr
        with worker.open("r+b") as stream:
            stream.write(first)
        manifest["protocol_version"] = 999
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        assert "identity/protocol mismatch" in run(exe, "package-check", ok=False).stderr
        assert "identity/protocol mismatch" in run(exe, "mcp", "--no-overlay", ok=False).stderr
        manifest = json.loads(original)
        contract = json.loads((isolated / "perception/runtime-contract.json").read_text(encoding="utf-8"))
        recognizer = next(model["name"] for model in contract["models"] if model["role"] == "ocr-recognize")
        del manifest["files"][f"perception/models/{recognizer}"]
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        assert "does not bind runtime resource" in run(exe, "package-check", ok=False).stderr
        manifest = json.loads(original)
        manifest["files"]["../outside"] = "0" * 64
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        assert "invalid package-relative path" in run(exe, "package-check", ok=False).stderr
        manifest = json.loads(original)
        del manifest["files"]["perception/msvcp140.dll"]
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        assert "required CRT resource" in run(exe, "package-check", ok=False).stderr
        manifest_path.write_bytes(original)
        # Normal Driver startup does not hash model payloads. Loading a damaged
        # model still fails through the upstream worker's own manifest validator.
        model = isolated / "perception/models" / recognizer
        with model.open("r+b") as stream:
            first = stream.read(1)
            stream.seek(0)
            stream.write(bytes([first[0] ^ 1]))
        try:
            test_mcp(isolated, cache)
            assert "sha256 mismatch" in run(
                worker, "--manifest", str(isolated / "perception/model-manifest.json"),
                "--onnx-runtime-library", str(isolated / "perception/runtime/onnxruntime.dll"),
                "--extension-id", "cua-perception", "--extension-version", manifest["product_version"],
                ok=False,
            ).stderr.lower()
        finally:
            with model.open("r+b") as stream:
                stream.write(first)
        for args in [("update", "--apply"), ("extension", "remove", "perception"), ("autostart", "enable")]:
            assert "managed together" in run(exe, *args, ok=False).stderr
        daemon = subprocess.Popen([str(exe), "serve", "--no-overlay"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                result = subprocess.run([str(exe), "status"], capture_output=True, text=True, encoding="utf-8", timeout=10)
                if result.returncode == 0:
                    assert "daemon is running" in result.stdout
                    break
                time.sleep(.1)
            else:
                raise AssertionError("native serve did not start")
        finally:
            run(exe, "stop")
            daemon.wait(timeout=15)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            stopped = subprocess.run([str(exe), "status"], capture_output=True, text=True, encoding="utf-8", timeout=10)
            if stopped.returncode != 0:
                assert "daemon is not running" in stopped.stderr, stopped.stderr
                break
            time.sleep(0.1)
        else:
            raise AssertionError("isolated daemon did not stop")
    return {"integrity": "passed", "negative_cases": 12, "startup_without_payload_hashing": "passed", "upstream_model_hash_rejection": "passed", "unicode_path": "passed", "daemon_start_status_stop": "passed"}


def test_mcp(bundle, cache):
    environment = dict(os.environ, CUA_WIN_STATE_DIR=str(cache / "mcp-state"), DO_NOT_TRACK="1")
    process = subprocess.Popen([str(bundle / "cua-win.exe"), "mcp", "--no-overlay"],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, encoding="utf-8", env=environment)
    responses = queue.Queue()
    errors = []

    def read_stdout():
        for line in process.stdout:
            try:
                responses.put(json.loads(line))
            except ValueError:
                responses.put({"invalid_stdout": line})

    def read_stderr():
        for line in process.stderr:
            if len(errors) < 30:
                errors.append(line.rstrip())

    threading.Thread(target=read_stdout, daemon=True).start()
    threading.Thread(target=read_stderr, daemon=True).start()

    def request(number, method, params):
        process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": number, "method": method, "params": params}) + "\n")
        process.stdin.flush()
        while True:
            response = responses.get(timeout=45)
            assert "invalid_stdout" not in response, response
            if response.get("id") == number:
                assert "error" not in response, response
                return response["result"]

    try:
        initialized = request(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "cua-win-package-test", "version": "1"}})
        assert initialized["protocolVersion"] == "2025-06-18", initialized
        process.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        process.stdin.flush()
        listed = request(2, "tools/list", {})
        names = {tool["name"] for tool in listed["tools"]}
        assert {"get_window_state", "parse_visual_regions", "start_recording", "list_windows"} <= names, names
        assert not ({"install_extension", "remove_extension", "update_extension"} & names), names
        windows = request(3, "tools/call", {"name": "list_windows", "arguments": {}})
        assert not windows.get("isError"), windows
        configured = request(4, "tools/call", {"name": "set_config", "arguments": {"key": "max_image_dimension", "value": 640}})
        assert not configured.get("isError"), configured
        saved = json.loads((cache / "mcp-state/config.json").read_text(encoding="utf-8"))
        assert saved["max_image_dimension"] == 640, saved
        # A second read proves the same live stdio connection remains usable.
        again = request(5, "tools/list", {})
        assert {tool["name"] for tool in again["tools"]} == names
        process.stdin.close()
        assert process.wait(timeout=15) == 0, errors
        return {"persistent_mcp": "passed", "tool_count": len(names), "read_only_windows": "passed", "isolated_config": "passed", "eof_shutdown": "passed"}
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        process.stdout.close()
        process.stderr.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("bundle", type=Path)
    args = parser.parse_args()
    bundle = args.bundle.resolve(strict=True)
    cache = Path(__file__).resolve().parent / ".cache" / "tests"
    cache.mkdir(parents=True, exist_ok=True)
    results = {**test_integrity(bundle, cache), **test_mcp(bundle, cache)}
    results["not_run"] = ["Windows 1903", "host input", "full Chinese OCR quality", "human teaching"]
    (cache / "package-results.json").write_text(json.dumps(results, indent=2), encoding="utf-8")
    print(json.dumps(results, ensure_ascii=False))


if __name__ == "__main__":
    main()
