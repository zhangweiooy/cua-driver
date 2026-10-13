"""Opt-in native CLI teaching through one persistent Driver runtime."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid
import ctypes as C
from test_teaching import Fixture

def main():
    p=argparse.ArgumentParser()
    p.add_argument("bundle",type=Path)
    p.add_argument("--allow-desktop",action="store_true",required=True)
    args=p.parse_args()
    exe=args.bundle.resolve()/"cua-win.exe"
    pipe="\\\\.\\pipe\\cua-teaching-test-"+uuid.uuid4().hex
    fixture=Fixture()
    with tempfile.TemporaryDirectory(prefix="cua-teaching-cli-") as temp:
        env=dict(os.environ,CUA_WIN_STATE_DIR=temp,DO_NOT_TRACK="1")
        server=subprocess.Popen([str(exe),"serve","--socket",pipe,"--no-overlay","--dangerously-bypass-approvals"],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,env=env)
        def call(name,data,expected=None):
            result=subprocess.run([str(exe),"call",name,"--socket",pipe,"--json"],input=json.dumps(data),text=True,encoding="utf-8",capture_output=True,env=env,timeout=30)
            if expected:
                assert result.returncode!=0 and expected in result.stdout+result.stderr,(result.stdout,result.stderr)
                return
            assert result.returncode==0,(name,result.stdout,result.stderr)
            return json.loads(result.stdout)
        try:
            deadline=time.monotonic()+15
            while time.monotonic()<deadline:
                if subprocess.run([str(exe),"status","--socket",pipe],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,env=env).returncode==0:break
                time.sleep(.1)
            else:raise AssertionError("serve not ready")
            base={"session":"cli-teaching","session_id":"persistent"}
            call("start_teaching",{"session_id":"disposable"},"teaching_requires_named_session")
            started=call("start_teaching",dict(base,output_dir=str(Path(temp)/"中文示教")))
            time.sleep(.2)
            assert call("get_teaching_state",base)["active"],"one-shot CLI exit ended teaching"
            call("type_text",{"session":"agent","pid":os.getpid(),"window_id":int(fixture.hwnd),"text":"must not appear"},"teaching_active")
            final=call("stop_teaching",base)
            assert final==call("stop_teaching",base)
            output=Path(final["output_dir"])
            assert output!=Path(temp)/"中文示教" and output.is_dir()
            saved=json.loads((output/"session.json").read_text(encoding="utf-8"))
            assert not saved["active"] and saved["cutoff_sequence"]==final["cutoff_sequence"]
            command=[str(exe),"call","type_text","--socket",pipe,"--json"]
            action=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,encoding="utf-8",env=env)
            fixture.focus()
            action.stdin.write(json.dumps({"session":"agent","pid":os.getpid(),"window_id":int(fixture.hwnd),"text":"1234567890"*30}))
            action.stdin.close()
            value=C.create_unicode_buffer(256)
            deadline=time.monotonic()+5
            while time.monotonic()<deadline:
                fixture.u.GetWindowTextW(fixture.edit,value,len(value))
                if value.value:break
                time.sleep(.02)
            else:raise AssertionError("automation did not start")
            call("start_teaching",dict(base,session_id="busy"),"teaching_busy")
            assert action.wait(timeout=15)==0,action.stderr.read()
            assert value.value
            print(json.dumps({"persistent_cli_owner":"passed","unnamed_cli_rejected":"passed","output_files":"passed","same_runtime_two_way_exclusion":"passed"}))
        finally:
            subprocess.run([str(exe),"stop","--socket",pipe],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,env=env)
            try:server.wait(timeout=15)
            except subprocess.TimeoutExpired:server.kill();server.wait()
            fixture.close()

if __name__=="__main__":main()
