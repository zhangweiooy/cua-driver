"""Opt-in real-IME fixture. Inputs come from the UI; this script injects none.

Run with --bundle and --output in a disposable test directory. Write command.json
with a new id and action (start/state/stop/checkpoint/quit); response.json and
checkpoint files retain the actual control values and teaching output.
"""
import argparse
import ctypes as C
from ctypes import wintypes as W
import json
from pathlib import Path
import time
from test_teaching import Fixture, Mcp


class ImeFixture(Fixture):
    def loop(self):
        self.u.GetKeyboardLayout.restype = C.c_void_p
        self.u.GetWindowThreadProcessId.argtypes = [W.HWND, C.c_void_p]
        self.hwnd = self.u.CreateWindowExW(
            0, "STATIC", "CUA real IME acceptance - disposable", 0x10CF0000,
            120, 120, 780, 400, None, None, None, None)
        assert self.hwnd, C.get_last_error()
        self.u.CreateWindowExW(0, "STATIC", "Field A - Pinyin / candidate / replacement",
            0x50000000, 20, 15, 700, 25, self.hwnd, None, None, None)
        self.edit = self.u.CreateWindowExW(0, "EDIT", "", 0x50810080,
            20, 50, 700, 55, self.hwnd, None, None, None)
        self.u.CreateWindowExW(0, "STATIC", "Field B - cross-control / paste",
            0x50000000, 20, 135, 700, 25, self.hwnd, None, None, None)
        self.edit_b = self.u.CreateWindowExW(0, "EDIT", "", 0x50810080,
            20, 170, 700, 55, self.hwnd, None, None, None)
        self.ready.set()
        msg = W.MSG()
        while self.u.GetMessageW(C.byref(msg), None, 0, 0) > 0:
            if msg.message == 0x8001:
                self.u.DestroyWindow(self.hwnd)
                break
            self.u.TranslateMessage(C.byref(msg))
            self.u.DispatchMessageW(C.byref(msg))

    def values(self):
        result = {}
        for key, hwnd in [("a", self.edit), ("b", self.edit_b)]:
            value = C.create_unicode_buffer(4096)
            self.u.GetWindowTextW(hwnd, value, len(value))
            result[key] = value.value
        thread = self.u.GetWindowThreadProcessId(self.hwnd, None)
        result["keyboard_layout"] = hex(self.u.GetKeyboardLayout(thread))
        result["at_epoch_ms"] = int(time.time() * 1000)
        return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--allow-desktop", action="store_true", required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        target = args.output / name
        temporary = target.with_suffix(".tmp")
        temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")
        temporary.replace(target)
    fixture = ImeFixture()
    client = None
    active = False
    try:
        client = Mcp(args.bundle.resolve(), args.output / "driver-state")
        save("ready.json", {"window": int(fixture.hwnd), "values": fixture.values()})
        previous = None
        deadline = time.monotonic() + 900
        while time.monotonic() < deadline:
            try:
                command = json.loads((args.output / "command.json").read_text(encoding="utf-8"))
            except (FileNotFoundError, json.JSONDecodeError):
                time.sleep(.1)
                continue
            if command["id"] == previous:
                time.sleep(.1)
                continue
            previous = command["id"]
            action = command["action"]
            result = {"id": previous, "values": fixture.values()}
            if action == "start":
                result["teaching"] = client.call("start_teaching", {
                    "session_id": "real-ime", "max_seconds": 600,
                    "output_dir": str(args.output.resolve() / "sessions")})
                active = True
            elif action in ("state", "stop"):
                result["teaching"] = client.call(
                    "get_teaching_state" if action == "state" else "stop_teaching",
                    {"session_id": "real-ime", "timeline_limit": 1000, "evidence_limit": 1000})
                if action == "stop":
                    active = False
            elif action == "quit":
                break
            elif action != "checkpoint":
                raise ValueError(action)
            save(f"{previous}.json", result)
            save("response.json", result)
    finally:
        if client:
            if active:
                save("final.json", client.call("stop_teaching", {"session_id": "real-ime", "timeline_limit": 1000}))
            client.close()
        fixture.close()


if __name__ == "__main__":
    main()
