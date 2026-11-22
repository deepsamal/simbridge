"""Grader for T3: CLINT machine-timer tick at 100 Hz on FE310. Hidden from the agent."""

from grading import count, mem_u32, trace_counts

# simbridge's fe310 preset runs mtime at the FE310's 32.768 kHz RTC clock
# (Renode's own platform says 62 MHz). 32768 / 100 = 327.68, so a firmware
# that rounds the period gains or loses a tick every few seconds: the 10 s
# check catches that; the 1 s and 2 s checks allow one tick either way.
CHECKS = {"start": 1, "tick_rate": 2, "long_run_exact": 1, "irq_source": 1, "quiet": 1}
MTIMECMP = (0x4000, 0x4004)
MTIME = (0xBFF8, 0xBFFC)


def grade(sb, elf, g):
    sb.call("load", elf=str(elf), platform="fe310")
    sb.call("peripheral_trace", peripheral="clint")  # enable before any code runs

    # 1. Start line.
    out = sb.call("run_until", uart_regex=r"T3 start\r\n", timeout_ms=50)
    g.check("start", out["hit"], weight=CHECKS["start"],
            detail={"elapsed_virtual_ms": out["elapsed_virtual_ms"], "uart": out["text"][-200:]})

    # 2. 100 +- 1 ticks after 1 s, 200 +- 1 after 2 s.
    sb.call("run_for", virtual_ms=1000 - out["virtual_time_s"] * 1000)
    t1 = mem_u32(sb, "TICKS")
    sb.call("run_for", virtual_ms=1000)
    t2 = mem_u32(sb, "TICKS")
    g.check("tick_rate", 99 <= t1 <= 101 and 199 <= t2 <= 201, weight=CHECKS["tick_rate"],
            detail={"TICKS_at_1s": t1, "TICKS_at_2s": t2})

    # 5. (read now, before the long run) The CLINT is reprogrammed about once a
    #    tick, and mtime isn't polled: a handful of reads at start-up at most.
    c = trace_counts(sb, "clint")
    cmp_w, mtime_r = count(c, "W", *MTIMECMP), count(c, "R", *MTIME)
    ok = t2 >= 150 and cmp_w >= t2 and mtime_r <= 20
    g.check("irq_source", ok, weight=CHECKS["irq_source"],
            detail={"mtimecmp_writes": cmp_w, "mtime_reads": mtime_r, "ticks": t2})

    # 3. Exactly 1000 ticks at 10.005 s: no drift from rounding 327.68.
    sb.call("run_for", virtual_ms=8005)
    t10 = mem_u32(sb, "TICKS")
    g.check("long_run_exact", t10 == 1000, weight=CHECKS["long_run_exact"],
            detail={"TICKS_at_10.005s": t10, "expected": 1000})

    # 4. Nothing printed after the start line.
    rest = sb.call("serial")["text"]
    g.check("quiet", out["hit"] and rest == "", weight=CHECKS["quiet"],
            detail={"after_start": rest[:200]})
