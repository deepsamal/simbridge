"""End-to-end smoke test of simbridge.renode against both firmwares (no MCP needed)."""
import json, sys, time
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from simbridge.renode import Renode

FW = {
    "stm32f4": ROOT / "firmware/stm32f4-hello/target/thumbv7em-none-eabihf/release/stm32f4-hello",
    "fe310": ROOT / "firmware/fe310-hello/target/riscv32imac-unknown-none-elf/release/fe310-hello",
}
TRACE = {"stm32f4": "usart2", "fe310": "uart0"}

def show(label, d):
    print("%-14s %s" % (label, json.dumps(d)[:300]))

r = Renode()
t0 = time.perf_counter(); r.start(); print("renode startup %.0f ms" % ((time.perf_counter()-t0)*1000))
try:
    for plat, elf in FW.items():
        print("=== %s" % plat)
        show("load", r.load(elf, plat))
        show("trace on", r.peripheral_trace(TRACE[plat]))
        t = time.perf_counter(); show("run_for 50", r.run_for(50)); print("   wall %.0f ms, vt %s" % ((time.perf_counter()-t)*1000, r.virtual_time_s()))
        s = r.serial(); show("serial", s); assert "hello from" in s["text"], s
        show("trace", r.peripheral_trace(TRACE[plat], limit=3))
        show("regs", r.regs(TRACE[plat], count=4))
        before = r.mem("TICKS", 4); show("mem TICKS", before)
        snap = r.snapshot(); show("snapshot", snap)
        r.run_for(50); after = r.mem("TICKS", 4); show("mem after", after)
        r.serial()
        show("restore", r.restore(snap["id"]))
        restored = r.mem("TICKS", 4); show("mem restored", restored)
        assert restored["hex"] == before["hex"], (restored, before)
        r.run_for(20); show("serial post", r.serial())
        show("reset", r.reset()); r.run_for(10); show("serial reset", r.serial())
    print("ALL OK")
finally:
    r.stop()
