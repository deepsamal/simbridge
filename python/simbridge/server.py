"""simbridge: MCP server exposing a headless Renode simulator to coding agents.

Every tool returns small structured JSON and reports `virtual_time_s` (the
machine's elapsed virtual time after the call) and `wall_ms` (host time the
call took).
"""

import atexit
import os
import threading
import time
from typing import Optional, Union

from mcp.server.fastmcp import FastMCP

from .renode import PLATFORMS, Renode, RenodeError

mcp = FastMCP("simbridge")
_lock = threading.Lock()
_renode: Optional[Renode] = None


def _r() -> Renode:
    global _renode
    if _renode is None:
        _renode = Renode(workdir=os.environ.get("SIMBRIDGE_WORKDIR"))
        atexit.register(_renode.stop)
    return _renode


def _call(fn, *args, **kwargs) -> dict:
    t = time.perf_counter()
    with _lock:
        r = _r()
        try:
            out = {"ok": True, **fn(r, *args, **kwargs)}
        except (RenodeError, OSError, ValueError) as e:
            out = {"ok": False, "error": str(e)}
        try:
            out["virtual_time_s"] = r.virtual_time_s() if r.loaded else 0.0
        except RenodeError:
            out["virtual_time_s"] = None
    out["wall_ms"] = round((time.perf_counter() - t) * 1000, 1)
    return out


@mcp.tool()
def load(elf: str, platform: str = "stm32f4", uart: Optional[str] = None) -> dict:
    """Create a fresh machine and load an ELF into it.

    platform: a preset (stm32f4 -> platforms/cpus/stm32f4.repl, console sysbus.usart2;
    fe310 / hifive1 -> platforms/cpus/sifive-fe310.repl, console sysbus.uart0) or a
    path to a .repl file (then `uart` is required). The machine starts paused at
    virtual time 0; use run_for to advance it.
    """
    return _call(lambda r: r.load(elf, platform, uart))


@mcp.tool()
def reset() -> dict:
    """Rebuild the machine from scratch and reload the same ELF (virtual time -> 0).
    Peripheral traces stay enabled; serial buffer is cleared."""
    return _call(lambda r: r.reset())


@mcp.tool()
def run_for(virtual_ms: float) -> dict:
    """Advance the simulation by exactly `virtual_ms` of virtual time (deterministic),
    then pause. Returns the program counter."""
    return _call(lambda r: r.run_for(float(virtual_ms)))


@mcp.tool()
def serial(max_bytes: int = 4096) -> dict:
    """UART console output captured since the previous serial() call (tail-truncated
    to max_bytes)."""
    return _call(lambda r: r.serial(max_bytes))


@mcp.tool()
def regs(peripheral: str, count: int = 8, offset: int = 0) -> dict:
    """Read `count` (<=64) 32-bit words from a peripheral's memory-mapped window,
    e.g. peripheral="usart2". Reads go through the model, so registers with read
    side effects (FIFOs, clear-on-read flags) are affected."""
    return _call(lambda r: r.regs(peripheral, count, offset))


@mcp.tool()
def mem(addr: Union[str, int], length: int = 64) -> dict:
    """Read up to 1024 bytes of memory. `addr` is a number ("0x20000000") or an ELF
    symbol name (e.g. "TICKS"). Returns hex."""
    return _call(lambda r: r.mem(addr, length))


@mcp.tool()
def peripheral_trace(peripheral: str, limit: int = 50, disable: bool = False) -> dict:
    """Bus accesses to a peripheral (via `sysbus LogPeripheralAccess`) logged since the
    previous call. The first call enables tracing and returns nothing; run_for, then
    call again. Returns the last `limit` accesses plus the total count."""
    return _call(lambda r: r.peripheral_trace(peripheral, limit, disable))


@mcp.tool()
def snapshot() -> dict:
    """Save full emulation state (Renode `Save`). Returns an id for restore() and the
    time the save took."""
    return _call(lambda r: r.snapshot())


@mcp.tool()
def restore(id: str) -> dict:
    """Restore a snapshot taken with snapshot() (Renode `Load`), including virtual
    time, CPU and memory state. Returns the time the load took."""
    return _call(lambda r: r.restore(id))


def main():
    mcp.run()  # stdio transport


if __name__ == "__main__":
    main()
