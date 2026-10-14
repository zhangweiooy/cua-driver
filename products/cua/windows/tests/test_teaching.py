"""Opt-in Windows native teaching/MCP acceptance. Python is test-only.

Uses isolated product state and a test-owned EDIT window. Verifies teaching
and automation are mutually exclusive, without sending input to user apps.
"""
import argparse
import ctypes as C
from ctypes import wintypes as W
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time


class Mcp:
    def __init__(self, bundle, state):
        self.process = subprocess.Popen([str(bundle / "cua-win.exe"), "mcp", "--no-overlay"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, encoding="utf-8", env=dict(os.environ, CUA_WIN_STATE_DIR=str(state), DO_NOT_TRACK="1",
                CUA_DRIVER_PERMISSION_MODE="unrestricted", CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS="1"))
        self.responses = queue.Queue()
        self.errors = []
        self.number = 0
        def read():
            for line in self.process.stdout:
                self.responses.put(json.loads(line))
        threading.Thread(target=read, daemon=True).start()
        def errors():
            for line in self.process.stderr:
                if len(self.errors) < 30:
                    self.errors.append(line)
        threading.Thread(target=errors, daemon=True).start()
        self.request("initialize", {"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"teaching-native-test","version":"1"}})
        self.process.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        self.process.stdin.flush()

    def request(self, method, params):
        self.number += 1
        self.process.stdin.write(json.dumps({"jsonrpc":"2.0","id":self.number,"method":method,"params":params})+"\n")
        self.process.stdin.flush()
        while True:
            reply = self.responses.get(timeout=30)
            if reply.get("id") == self.number:
                assert "error" not in reply, reply
                return reply["result"]

    def call(self, name, arguments, error=None):
        result = self.request("tools/call", {"name":name,"arguments":arguments})
        data = result.get("structuredContent")
        if error:
            assert result.get("isError") and data and (data.get("error", {}).get("code") or data.get("refusal", {}).get("code")) == error, result
        else:
            assert not result.get("isError"), result
            assert data is not None, result
        return data

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                assert self.process.wait(timeout=10) == 0, self.errors
            finally:
                if self.process.poll() is None:
                    self.process.kill()
                    self.process.wait(timeout=5)


class Fixture:
    def __init__(self):
        self.u = C.WinDLL("user32", use_last_error=True)
        self.u.CreateWindowExW.restype = W.HWND
        self.u.CreateWindowExW.argtypes = [W.DWORD,W.LPCWSTR,W.LPCWSTR,W.DWORD,C.c_int,C.c_int,C.c_int,C.c_int,W.HWND,W.HMENU,W.HINSTANCE,C.c_void_p]
        self.u.DefWindowProcW.restype = C.c_ssize_t
        self.u.DefWindowProcW.argtypes = [W.HWND,W.UINT,W.WPARAM,W.LPARAM]
        self.u.SendMessageW.restype = C.c_ssize_t
        self.u.SendMessageW.argtypes = [W.HWND,W.UINT,W.WPARAM,W.LPARAM]
        self.u.PostMessageW.argtypes = [W.HWND,W.UINT,W.WPARAM,W.LPARAM]
        self.u.SetForegroundWindow.argtypes = [W.HWND]
        self.u.GetForegroundWindow.restype = W.HWND
        self.u.SetFocus.argtypes = [W.HWND]
        self.u.DestroyWindow.argtypes = [W.HWND]
        self.u.GetWindowTextW.argtypes = [W.HWND,W.LPWSTR,C.c_int]
        self.ready = threading.Event()
        self.focused = threading.Event()
        self.thread = threading.Thread(target=self.loop)
        self.thread.start()
        assert self.ready.wait(5)

    def loop(self):
        self.hwnd = self.u.CreateWindowExW(0,"STATIC","CUA teaching native test — disposable",0x10CF0000,120,120,620,240,None,None,None,None)
        assert self.hwnd, C.get_last_error()
        self.edit = self.u.CreateWindowExW(0,"EDIT","",0x50810000,20,40,540,70,self.hwnd,None,None,None)
        self.u.SetForegroundWindow(self.hwnd)
        self.u.SetFocus(self.edit)
        self.ready.set()
        msg = W.MSG()
        while self.u.GetMessageW(C.byref(msg),None,0,0)>0:
            if msg.message == 0x8001:
                self.u.DestroyWindow(self.hwnd)
                break
            if msg.message == 0x8002:
                self.u.SetForegroundWindow(self.hwnd)
                self.u.SetFocus(self.edit)
                self.focused.set()
                continue
            self.u.TranslateMessage(C.byref(msg))
            self.u.DispatchMessageW(C.byref(msg))

    def focus(self):
        self.focused.clear()
        self.u.PostMessageW(self.hwnd,0x8002,0,0)
        assert self.focused.wait(5)

    def close(self):
        self.u.PostMessageW(self.hwnd,0x8001,0,0)
        self.thread.join(timeout=5)
        assert not self.thread.is_alive()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("bundle", type=Path)
    p.add_argument("--allow-desktop", action="store_true", required=True)
    a = p.parse_args()
    bundle = a.bundle.resolve(strict=True)
    fixture = Fixture()
    clients = []
    results = {}
    try:
        with tempfile.TemporaryDirectory(prefix="cua-teaching-test-") as state:
            def client():
                c = Mcp(bundle, Path(state))
                clients.append(c)
                return c
            first, second = client(), client()
            names = {t["name"] for t in first.request("tools/list", {})["tools"]}
            assert {"start_teaching","get_teaching_state","stop_teaching","run_steps","run_script"} <= names
            first.call("start_teaching", {"session_id":"native-one","max_seconds":10})
            first.call("start_teaching", {"session_id":"duplicate"}, "teaching_already_active")
            second.call("start_teaching", {"session_id":"independent"})
            second.call("stop_teaching", {"session_id":"independent"})
            first.call("stop_teaching", {"session_id":"wrong"}, "teaching_session_mismatch")
            first.call("get_teaching_state", {"session_id":"native-one","session":"another-owner"}, "teaching_owner_mismatch")
            fixture.u.SetForegroundWindow(fixture.hwnd)
            time.sleep(.2)
            arguments = {"pid":os.getpid(),"window_id":int(fixture.hwnd),"text":"互斥验证","delay_ms":10}
            first.call("type_text", arguments, "teaching_active")
            # New 0.35 entry points and its hidden batch alias must use the same
            # runtime admission gate before dispatching any nested automation.
            for tool in ("run_steps", "run_actions"):
                first.call(tool, {"steps":[{"tool":"type_text","args":arguments}]}, "teaching_active")
            first.call("run_script", {"script":"return 7;"}, "teaching_active")
            first.call("list_windows", {})
            actual = C.create_unicode_buffer(256)
            fixture.u.GetWindowTextW(fixture.edit, actual, len(actual))
            assert actual.value == "", repr(actual.value)
            final = first.call("stop_teaching", {"session_id":"native-one"})
            again = first.call("stop_teaching", {"session_id":"native-one"})
            assert final == again, (final, again)
            assert final["evidence"]["frozen"] and final["evidence"]["pending"] == 0
            assert final["evidence"]["cutoff_sequence"] is not None
            drained = first.call("get_teaching_state", {"session_id":"native-one","after_evidence_sequence":final["evidence"]["cutoff_sequence"],"evidence_limit":1})
            assert drained["evidence"]["entries"] == []
            assert final["cutoff_sequence"] == final["action_count"]
            assert not any(x["actionType"] == "typeText" for x in final["actions"]), final
            fixture.focus()
            second.call("type_text", arguments)
            fixture.u.GetWindowTextW(fixture.edit, actual, len(actual))
            assert actual.value == "互斥验证", repr(actual.value)
            results["runtime_local_exclusion_and_independent_runtime"] = "passed"
            results["read_only_allowed_and_automation_restored_after_stop"] = "passed"
            output = Path(final["output_dir"])
            saved = json.loads((output / "session.json").read_text(encoding="utf-8"))
            assert saved["cutoff_sequence"] == final["cutoff_sequence"]
            assert (output / "complete.json").exists() == final["complete"]
            first.call("start_teaching", {"session_id":"cancel-me"})
            cancelling = first.call("get_teaching_state", {"session_id":"cancel-me"})
            cancelled_output = Path(cancelling["output_dir"])
            (cancelled_output / "caller-note.txt").write_text("keep", encoding="utf-8")
            cancelled = first.call("stop_teaching", {"session_id":"cancel-me","cancel":True})
            assert cancelled["cancelled"]
            assert cancelled == first.call("stop_teaching", {"session_id":"cancel-me","cancel":True})
            assert (cancelled_output / "caller-note.txt").read_text() == "keep"
            assert not (cancelled_output / "session.json").exists()
            assert (output / "session.json").exists()
            results["output_files_completion_marker_and_bounded_cancel"] = "passed"
            results["owner_session_single_desktop_and_stable_stop"] = "passed"
            first.call("start_teaching", {"session_id":"native-timeout","max_seconds":1})
            time.sleep(1.5)
            timed = first.call("get_teaching_state", {"session_id":"native-timeout"})
            assert not timed["active"] and timed["termination_reason"] == "time_limit" and not timed["complete"], timed
            results["safety_timer"] = "passed"
            disconnecting = first.call("start_teaching", {"session_id":"native-disconnect","max_seconds":10})
            first.close()
            disconnected = json.loads((Path(disconnecting["output_dir"]) / "session.json").read_text(encoding="utf-8"))
            assert not disconnected["active"] and disconnected["termination_reason"] == "owner_disconnected", disconnected
            assert disconnected["evidence"]["frozen"] and not disconnected["complete"]
            second.call("start_teaching", {"session_id":"after-disconnect","max_seconds":10})
            second.call("stop_teaching", {"session_id":"after-disconnect"})
            results["disconnect_releases_hooks_and_desktop_owner"] = "passed"
    finally:
        for c in clients:
            c.close()
        fixture.close()
    results["not_run"] = ["physical human input and real IME", "secure desktop transition", "Windows 1903"]
    out = Path(__file__).parent / ".cache/tests/teaching-results.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(results, indent=2), encoding="utf-8")
    print(json.dumps(results))


if __name__ == "__main__":
    main()
