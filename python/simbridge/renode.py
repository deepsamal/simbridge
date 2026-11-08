"""Thin client for a headless Renode instance driven through its monitor socket.

Works on Python 3.8+ with only the standard library, so it can be tested and
reused independently of the MCP layer in server.py.
"""

import os
import re
import shutil
import socket
import subprocess
import tempfile
import threading
import time
from pathlib import Path

PROMPT_RE = re.compile(rb"\r?\(([^()\r\n]+)\) $")
IAC_RE = re.compile(rb"\xff[\xfb-\xfe].|\xff[\xf0-\xfa]")
ERROR_MARKERS = (
    "There was an error",
    "Could not tokenize",
    "No such command",
    "does not provide a field",
    "Could not find",
    "Exception",
)
VT_RE = re.compile(r"Elapsed Virtual Time: (\d+):(\d+):(\d+(?:\.\d+)?)")
TRACE_RE = re.compile(
    r"\[cpu: (0x[0-9A-Fa-f]+)\] (Read|Write)(\w+) (?:from|to) (0x[0-9A-Fa-f]+)"
    r"(?: \(([^)]*)\))?,? (?:returned|value) (0x[0-9A-Fa-f]+)"
)

MACHINE = "sb"

# Built-in platform presets: repl shipped with Renode + the console UART.
PLATFORMS = {
    "stm32f4": {"repl": "platforms/cpus/stm32f4.repl", "uart": "sysbus.usart2"},
    "fe310": {"repl": "platforms/cpus/sifive-fe310.repl", "uart": "sysbus.uart0"},
}
PLATFORMS["hifive1"] = PLATFORMS["fe310"]


class RenodeError(RuntimeError):
    pass


def _q(path):
    """Quote a host path for the monitor (spaces break the @path tokenizer)."""
    return '"%s"' % str(Path(path).resolve()).replace("\\", "/")


def _free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def find_renode():
    env = os.environ.get("SIMBRIDGE_RENODE")
    if env:
        return env
    for cand in (
        shutil.which("renode"),
        r"C:\Program Files\Renode\renode.exe",
        "/opt/renode/renode",
        "/Applications/Renode.app/Contents/MacOS/macos_run.command",
    ):
        if cand and Path(cand).exists():
            return cand
    raise RenodeError("Renode not found; set SIMBRIDGE_RENODE to the renode executable")


class UartTap:
    """Connects to a Renode ServerSocketTerminal and buffers everything it sends."""

    def __init__(self, port):
        self._buf = bytearray()
        self._lock = threading.Lock()
        self.dropped = 0
        deadline = time.time() + 5
        while True:
            try:
                self._sock = socket.create_connection(("127.0.0.1", port), timeout=1)
                break
            except OSError:
                if time.time() > deadline:
                    raise
                time.sleep(0.05)
        self._sock.settimeout(0.2)
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        while True:
            try:
                d = self._sock.recv(4096)
                if not d:
                    return
                with self._lock:
                    self._buf += d
                    if len(self._buf) > 1 << 20:  # keep last 1 MiB
                        cut = len(self._buf) - (1 << 20)
                        del self._buf[:cut]
                        self.dropped += cut
            except socket.timeout:
                continue
            except OSError:
                return

    def take(self):
        time.sleep(0.05)  # let in-flight bytes land after a RunFor
        with self._lock:
            data = bytes(self._buf)
            self._buf.clear()
            dropped, self.dropped = self.dropped, 0
        return data, dropped

    def close(self):
        try:
            self._sock.close()
        except OSError:
            pass


class Renode:
    def __init__(self, renode_path=None, workdir=None):
        self.renode_path = renode_path or find_renode()
        self.workdir = Path(workdir or tempfile.mkdtemp(prefix="simbridge-"))
        self.workdir.mkdir(parents=True, exist_ok=True)
        self.log_path = self.workdir / "renode.log"
        self.proc = None
        self.sock = None
        self.uart = None
        self.loaded = None  # dict(elf, repl, uart)
        self.traced = set()
        self.log_offsets = {}
        self.snapshots = {}
        self._term_seq = 0

    # ---- process / monitor -------------------------------------------------
    def start(self):
        if self.proc and self.proc.poll() is None:
            return
        port = _free_port()
        self.proc = subprocess.Popen(
            [self.renode_path, "--disable-xwt", "--plain", "--port", str(port)],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        deadline = time.time() + 60
        while True:
            try:
                self.sock = socket.create_connection(("127.0.0.1", port), timeout=1)
                break
            except OSError:
                if self.proc.poll() is not None or time.time() > deadline:
                    raise RenodeError("Renode monitor did not come up on port %d" % port)
                time.sleep(0.2)
        self._read_until_prompt(timeout=60)
        self.cmd("logFile %s" % _q(self.log_path))

    def stop(self):
        if self.uart:
            self.uart.close()
        if self.sock:
            try:
                self.sock.sendall(b"quit\n")
            except OSError:
                pass
            self.sock.close()
        if self.proc and self.proc.poll() is None:
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()

    def _read_until_prompt(self, timeout):
        self.sock.settimeout(0.5)
        out = b""
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                chunk = self.sock.recv(65536)
            except socket.timeout:
                continue
            if not chunk:
                raise RenodeError("Renode monitor closed the connection")
            out += chunk
            clean = IAC_RE.sub(b"", out)
            if PROMPT_RE.search(clean):
                return clean
        raise RenodeError("timed out waiting for Renode prompt")

    def cmd(self, line, timeout=120, check=True):
        """Run one monitor command; return its output with echo and prompt stripped."""
        self.start()
        self.sock.sendall(line.encode() + b"\n")
        raw = self._read_until_prompt(timeout)
        text = PROMPT_RE.sub(b"", raw).decode("utf-8", "replace")
        text = text.replace("\r", "")
        # Drop the echoed command (and any stale prompt in front of it).
        i = text.find(line)
        if i >= 0:
            text = text[i + len(line):]
        out = "\n".join(l for l in text.split("\n") if l.strip()).strip()
        if check and any(m in out for m in ERROR_MARKERS):
            raise RenodeError("`%s` failed: %s" % (line, out[:500]))
        return out

    # ---- helpers -----------------------------------------------------------
    def virtual_time_s(self):
        out = self.cmd("machine ElapsedVirtualTime")
        m = VT_RE.search(out)
        if not m:
            return None
        h, mi, s = m.groups()
        return round(int(h) * 3600 + int(mi) * 60 + float(s), 9)

    def pc(self):
        try:
            return self.cmd("sysbus.cpu PC")
        except RenodeError:
            return None

    def _attach_uart(self):
        if self.uart:
            self.uart.close()
        self._term_seq += 1
        port = _free_port()
        name = "sbterm%d" % self._term_seq
        self.cmd('emulation CreateServerSocketTerminal %d "%s" false' % (port, name))
        self.cmd("connector Connect %s %s" % (self.loaded["uart"], name))
        self.uart = UartTap(port)

    def _require_loaded(self):
        if not self.loaded:
            raise RenodeError("no machine loaded; call load() first")

    # ---- operations --------------------------------------------------------
    def load(self, elf, platform="stm32f4", uart=None):
        elf = Path(elf)
        if not elf.exists():
            raise RenodeError("ELF not found: %s" % elf)
        preset = PLATFORMS.get(platform.lower())
        if preset:
            repl, uart = preset["repl"], uart or preset["uart"]
            repl_arg = "@" + repl
        else:
            repl, repl_arg = platform, _q(platform)
            if not uart:
                raise RenodeError("custom platform needs an explicit uart (e.g. sysbus.usart1)")
        self.cmd("Clear")  # drop all machines (`mach clear` only deselects)
        self.traced.clear()
        self.cmd('mach create "%s"' % MACHINE)
        self.cmd("machine LoadPlatformDescription %s" % repl_arg)
        self.cmd("sysbus LoadELF %s" % _q(elf))
        self.loaded = {"elf": str(elf.resolve()), "platform": platform, "repl": repl, "uart": uart}
        self._attach_uart()
        return dict(self.loaded, pc=self.pc())

    def reset(self):
        """Deterministic reset: rebuild the machine from scratch and reload the ELF."""
        self._require_loaded()
        l = self.loaded
        traced = set(self.traced)
        info = self.load(l["elf"], l["platform"], l["uart"])
        for p in traced:
            self.enable_trace(p)
        return info

    def run_for(self, virtual_ms):
        self._require_loaded()
        self.cmd('emulation RunFor "%.9f"' % (virtual_ms / 1000.0), timeout=3600)
        return {"pc": self.pc()}

    def serial(self, max_bytes=4096):
        self._require_loaded()
        data, dropped = self.uart.take()
        truncated = max(0, len(data) - max_bytes)
        if truncated:
            data = data[-max_bytes:]
        return {
            "uart": self.loaded["uart"],
            "text": data.decode("utf-8", "replace"),
            "bytes": len(data),
            "truncated_bytes": truncated + dropped,
        }

    def peripheral_range(self, peripheral):
        name = peripheral if peripheral.startswith("sysbus.") else "sysbus." + peripheral
        out = self.cmd("sysbus GetRegistrationPoints %s" % name)
        m = re.search(r"<(0x[0-9A-Fa-f]+), (0x[0-9A-Fa-f]+)>", out)
        if not m:
            raise RenodeError("no bus registration for %s: %s" % (name, out[:200]))
        return int(m.group(1), 16), int(m.group(2), 16)

    def resolve_addr(self, addr):
        if isinstance(addr, int):
            return addr
        s = str(addr).strip()
        if re.fullmatch(r"(0x)?[0-9A-Fa-f]+", s) and (s.startswith("0x") or s.isdigit()):
            return int(s, 0)
        out = self.cmd('sysbus GetSymbolAddress "%s"' % s)
        return int(out.split()[0], 16)

    def regs(self, peripheral, count=8, offset=0):
        """Read `count` 32-bit words from the peripheral's bus window.

        Note: reads go through the model, so registers with read side effects
        (e.g. a UART data register) behave as if the CPU read them.
        """
        self._require_loaded()
        start, end = self.peripheral_range(peripheral)
        count = max(1, min(int(count), 64, (end - start + 1 - offset) // 4))
        words = []
        for i in range(count):
            a = start + offset + 4 * i
            v = self.cmd("sysbus ReadDoubleWord 0x%X" % a)
            words.append({"offset": "0x%X" % (offset + 4 * i), "value": v.split()[0]})
        return {"peripheral": peripheral, "base": "0x%08X" % start, "size": end - start + 1, "words": words}

    def mem(self, addr, length=64):
        self._require_loaded()
        a = self.resolve_addr(addr)
        length = max(1, min(int(length), 1024))
        out = self.cmd("sysbus ReadBytes 0x%X %d" % (a, length))
        data = bytes(int(b, 16) for b in re.findall(r"0x([0-9A-Fa-f]{2})\b", out))
        return {"addr": "0x%08X" % a, "len": len(data), "hex": data.hex()}

    def enable_trace(self, peripheral):
        name = peripheral if peripheral.startswith("sysbus.") else "sysbus." + peripheral
        self.cmd("sysbus LogPeripheralAccess %s true" % name)
        short = name.split(".", 1)[1]
        self.traced.add(short)
        self.log_offsets[short] = self._log_size()

    def _log_size(self):
        try:
            return self.log_path.stat().st_size
        except OSError:
            return 0

    def peripheral_trace(self, peripheral, limit=50, disable=False):
        """Return accesses logged since the previous call (first call enables tracing)."""
        self._require_loaded()
        short = peripheral.split(".", 1)[1] if peripheral.startswith("sysbus.") else peripheral
        if short not in self.traced:
            self.enable_trace(short)
            return {"peripheral": short, "enabled": True, "accesses": [], "total": 0,
                    "note": "tracing enabled; run_for() then call again"}
        start = self.log_offsets.get(short, 0)
        with open(self.log_path, "rb") as f:
            f.seek(start)
            chunk = f.read()
        self.log_offsets[short] = start + len(chunk)
        # Renode tags lines "usart2:" with one machine, "sb/usart2:" with several.
        tag = re.compile(r"[\] /]%s: \[cpu" % re.escape(short))
        accesses = []
        for line in chunk.decode("utf-8", "replace").splitlines():
            if not tag.search(line):
                continue
            m = TRACE_RE.search(line)
            if m:
                pc, op, width, off, reg, val = m.groups()
                accesses.append({"pc": pc, "op": op[0], "width": width, "offset": off,
                                 "reg": reg, "value": val})
        total = len(accesses)
        if disable:
            self.cmd("sysbus LogPeripheralAccess sysbus.%s false" % short)
            self.traced.discard(short)
        return {"peripheral": short, "total": total, "returned": min(total, limit),
                "accesses": accesses[-limit:]}

    def snapshot(self):
        self._require_loaded()
        sid = "s%d" % (len(self.snapshots) + 1)
        path = self.workdir / ("%s.save" % sid)
        t = time.perf_counter()
        self.cmd("Save %s" % _q(path))
        save_ms = (time.perf_counter() - t) * 1000
        self.snapshots[sid] = {"path": path, "loaded": dict(self.loaded), "traced": set(self.traced)}
        return {"id": sid, "save_ms": round(save_ms, 1), "size_bytes": path.stat().st_size}

    def restore(self, sid):
        snap = self.snapshots.get(sid)
        if not snap:
            raise RenodeError("unknown snapshot %r (have: %s)" % (sid, ", ".join(self.snapshots) or "none"))
        t = time.perf_counter()
        self.cmd("Load %s" % _q(snap["path"]))
        self.cmd('mach set "%s"' % MACHINE)
        load_ms = (time.perf_counter() - t) * 1000
        self.loaded = dict(snap["loaded"])
        # Terminals are transient externals: not saved, so re-attach.
        self._attach_uart()
        self.traced.clear()
        for p in snap["traced"]:
            self.enable_trace(p)
        total_ms = (time.perf_counter() - t) * 1000
        return {"id": sid, "load_ms": round(load_ms, 1), "restore_total_ms": round(total_ms, 1),
                "pc": self.pc()}
