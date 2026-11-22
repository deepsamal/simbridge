"""Grading helpers shared by every task's hidden/grade.py, and the CLI that runs one.

A task's grade.py defines:

    def grade(sb, elf, g):      # sb: SimBridge in grader mode, elf: Path, g: Grade
        out = sb.call("run_until", uart_regex="ready", timeout_ms=100)
        g.check("banner", out["hit"], detail=out["text"])

Run it on a firmware ELF:

    python bench/grading.py bench/tasks/B1-uart-banner-stm32f4 path/to/firmware.elf --out result.json
"""

import argparse
import importlib.util
import json
import sys
import time
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from simbridge_client import SimBridge, SimBridgeError  # noqa: E402


class Grade:
    """Collects weighted pass/fail checks. A task passes only if every check passes;
    `score` is the weighted fraction passed (partial credit)."""

    def __init__(self, task_id):
        self.task_id = task_id
        self.checks = []
        self.error = None

    def check(self, name, passed, detail=None, weight=1.0):
        self.checks.append({"name": name, "passed": bool(passed), "weight": weight,
                            "detail": detail if detail is None or isinstance(detail, (str, int, float, bool, list, dict))
                            else str(detail)})
        return bool(passed)

    def fail_remaining(self, checks, reason):
        """Record checks that couldn't run (e.g. the firmware never booted) as failed.
        `checks` is the grader's CHECKS: {name: weight}, or a list of names (weight 1)."""
        done = {c["name"] for c in self.checks}
        weights = checks if isinstance(checks, dict) else {n: 1.0 for n in checks}
        for n, w in weights.items():
            if n not in done:
                self.check(n, False, detail=f"not run: {reason}", weight=w)

    @property
    def passed(self):
        return bool(self.checks) and self.error is None and all(c["passed"] for c in self.checks)

    @property
    def score(self):
        total = sum(c["weight"] for c in self.checks)
        return round(sum(c["weight"] for c in self.checks if c["passed"]) / total, 4) if total else 0.0

    def result(self, elf, wall_s):
        return {"task": self.task_id, "elf": str(elf), "passed": self.passed, "score": self.score,
                "checks": self.checks, "error": self.error, "wall_s": round(wall_s, 2)}


# ---- helpers for grade.py ------------------------------------------------------

def hexint(s):
    return int(s, 16) if isinstance(s, str) else int(s)


def reg_value(sb, peripheral, register):
    """Value of one register by SVD name, plus its decoded fields as a dict."""
    out = sb.call("regs", peripheral=peripheral, register=register)
    for w in out["words"]:
        for r in w.get("registers", []):
            if r["register"].lower() == register.lower():
                return hexint(r["value"]), {f["name"]: f["value"] for f in r.get("fields", [])}
    raise SimBridgeError(f"{peripheral}.{register} not decoded (is the SVD loaded?)")


def trace_counts(sb, peripheral):
    """Reads and writes since the last trace read, as {offset: {"R": n, "W": m}}
    with offsets like "0x10". Consumes the log, like every trace read."""
    return sb.call("peripheral_trace", peripheral=peripheral, limit=0)["counts"]


def count(counts, op, *offsets):
    """Sum of `op` ("R"/"W") accesses at the given offsets in a trace_counts() result."""
    return sum(counts.get(hex(o).upper().replace("0X", "0x"), {}).get(op, 0) for o in offsets)


def mem_u32(sb, addr):
    """One little-endian 32-bit word from memory or a peripheral (address or symbol)."""
    return int.from_bytes(bytes.fromhex(sb.call("mem", addr=addr, length=4)["hex"]), "little")


def last_written(accesses):
    """{offset: last value written} from trace accesses. Use it to grade
    configuration: Renode models often don't keep registers they don't act on
    (nRF52840 UARTE BAUDRATE, FE310 UART div), so reading them back with mem()
    or regs() doesn't return what the firmware wrote."""
    out = {}
    for a in accesses:
        if a["op"] == "W":
            out[hexint(a["offset"])] = hexint(a["value"])
    return out


def trace_all(sb, peripheral, op=None, offset=None):
    """Every access logged since tracing was enabled or last read, optionally only
    reads/writes (op="R"/"W") at one register offset. Filter here rather than in
    Python: a firmware spinning on a status register logs millions of reads."""
    args = {k: v for k, v in (("op", op), ("offset", offset)) if v is not None}
    return sb.call("peripheral_trace", peripheral=peripheral, limit=1_000_000, **args)["accesses"]


# Offset of each board's console TX data register, for uart_tx().
UART_TX_OFFSET = {
    "stm32f4": 0x04, "stm32f103": 0x04,   # USART DR
    "stm32f072": 0x28, "stm32l552": 0x28,  # USART TDR
    "fe310": 0x00, "hifive1": 0x00, "k210": 0x00,  # SiFive txdata
}


def uart_tx(accesses, board):
    """(t, byte) for every write to the board's UART TX data register, in order.
    `t` is virtual seconds (at most one quantum early; see time_resolution_s).
    Not for nrf52840, whose UARTE sends by DMA: its bytes aren't in the trace."""
    if board not in UART_TX_OFFSET:
        raise ValueError(f"uart_tx has no TX register for board {board!r}")
    off = UART_TX_OFFSET[board]
    return [(a.get("t"), hexint(a["value"]) & 0xFF) for a in accesses
            if a["op"] == "W" and hexint(a["offset"]) == off]


def line_times(tx):
    """Group uart_tx() output into lines: [(t_of_last_byte, text)], split on \\n."""
    lines, cur = [], bytearray()
    for t, b in tx:
        if b == 0x0A:
            lines.append((t, cur.decode("ascii", "replace").rstrip("\r")))
            cur = bytearray()
        else:
            cur.append(b)
    return lines


# ---- runner --------------------------------------------------------------------

def load_task_grader(task_dir):
    path = Path(task_dir) / "hidden" / "grade.py"
    spec = importlib.util.spec_from_file_location(f"grade_{Path(task_dir).name}", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    if not hasattr(mod, "grade"):
        raise SystemExit(f"{path} defines no grade(sb, elf, g)")
    return mod


def run(task_dir, elf, exe=None):
    """Grade one ELF in a fresh grader-mode simbridge; never raises for firmware faults."""
    task_dir, elf = Path(task_dir), Path(elf).resolve()
    mod = load_task_grader(task_dir)
    g = Grade(task_dir.name)
    t = time.time()
    try:
        with SimBridge(exe=exe, mode="grader") as sb:
            mod.grade(sb, elf, g)
    except Exception as e:  # a crash in grading is recorded, and the task fails
        g.error = f"{type(e).__name__}: {e}"
        g.fail_remaining(getattr(mod, "CHECKS", []), g.error)
        if not isinstance(e, SimBridgeError):
            g.error += "\n" + traceback.format_exc(limit=3)
    return g.result(elf, time.time() - t)


def main():
    ap = argparse.ArgumentParser(description="Grade a firmware ELF against a simbridge-bench task.")
    ap.add_argument("task_dir")
    ap.add_argument("elf")
    ap.add_argument("--out", help="write result.json here")
    ap.add_argument("--simbridge", help="simbridge executable (default: SIMBRIDGE_EXE or target/release)")
    a = ap.parse_args()
    res = run(a.task_dir, a.elf, a.simbridge)
    text = json.dumps(res, indent=2)
    if a.out:
        Path(a.out).write_text(text, encoding="utf-8")
    print(text)
    sys.exit(0 if res["passed"] else 1)


if __name__ == "__main__":
    main()
