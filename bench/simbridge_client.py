"""Minimal MCP stdio client for simbridge, standard library only.

The grader uses it to drive its own simbridge instance in grader mode, separate
from whatever instance the agent used.

    with SimBridge() as sb:
        sb.call("load", elf="...", platform="stm32f4")
        sb.call("run_for", virtual_ms=10)
        print(sb.call("serial")["text"])
"""

import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def default_exe() -> Path:
    """`SIMBRIDGE_EXE`, else the repo's release build."""
    if os.environ.get("SIMBRIDGE_EXE"):
        return Path(os.environ["SIMBRIDGE_EXE"])
    exe = REPO / "target" / "release" / "simbridge"
    return exe.with_suffix(".exe") if sys.platform == "win32" else exe


class SimBridgeError(RuntimeError):
    pass


class SimBridge:
    def __init__(self, exe=None, mode="grader", tools=None, log=None, extra_env=None):
        env = dict(os.environ)
        env["SIMBRIDGE_MODE"] = mode
        # The grader sees every tool unless told otherwise, whatever the caller's env says.
        env.pop("SIMBRIDGE_TOOLS", None)
        env.pop("SIMBRIDGE_LOG", None)
        if tools:
            env["SIMBRIDGE_TOOLS"] = tools
        if log:
            env["SIMBRIDGE_LOG"] = str(log)
        env.update(extra_env or {})
        exe = Path(exe) if exe else default_exe()
        if not exe.is_file():
            raise SimBridgeError(f"simbridge not found at {exe}; run `cargo build --release` first")
        self.proc = subprocess.Popen(
            [str(exe)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, encoding="utf-8", env=env,
        )
        self._id = 0
        self._request("initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "simbridge-bench-grader", "version": "0.1"},
        })
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        self.tools = sorted(t["name"] for t in self._request("tools/list", {})["tools"])

    def _send(self, msg):
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def _request(self, method, params):
        self._id += 1
        rid = self._id
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        while True:
            line = self.proc.stdout.readline()
            if not line:
                err = self.proc.stderr.read()
                raise SimBridgeError(f"simbridge exited during {method}: {err.strip()[:500]}")
            msg = json.loads(line)
            if msg.get("id") != rid:
                continue  # notifications or log messages
            if "error" in msg:
                raise SimBridgeError(f"{method}: {msg['error'].get('message')}")
            return msg["result"]

    def call(self, tool, check=True, **args):
        """Call a tool and return its JSON result. With check=True (default) a
        result with ok:false raises, so a grader never mistakes an error for data."""
        res = self._request("tools/call", {"name": tool, "arguments": args})
        text = "".join(c.get("text", "") for c in res.get("content", []))
        try:
            out = json.loads(text)
        except json.JSONDecodeError:
            raise SimBridgeError(f"{tool}: non-JSON result: {text[:300]}")
        if check and out.get("ok") is False:
            raise SimBridgeError(f"{tool}: {out.get('error')}")
        return out

    def close(self):
        if self.proc.poll() is None:
            try:
                self.proc.stdin.close()
                self.proc.wait(timeout=15)
            except Exception:
                self.proc.kill()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()
