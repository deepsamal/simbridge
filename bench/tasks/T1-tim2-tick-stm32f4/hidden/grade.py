"""Grader for T1: TIM2 1 kHz tick on STM32F4. Hidden from the agent."""

from grading import line_times, reg_value, trace_all, uart_tx

TIMER_HZ = 16_000_000   # TIM2 clock at reset (16 MHz HSI, APB1 /1)
RUN_S = 3.5             # virtual seconds to observe: start + three count lines
LINES = ["T1 start", "T1 count=1000", "T1 count=2000", "T1 count=3000"]
# Renode's STM32_Timer reloads after ARR counts, not ARR + 1 as the hardware does,
# so firmware that is exact on hardware runs 1/(ARR) fast here: 0.1% at ARR = 999.
# The measured interval is checked to +-0.2%, and the rate the registers would give
# on real hardware is checked exactly (rate_config) -- otherwise ARR = 1000, an
# off-by-one on hardware, would look perfect in simulation.
INTERVAL_TOL = 0.002
CHECKS = {"first_count": 1, "counts": 2, "interval_1s": 2, "rate_config": 1,
          "isr_source": 1, "ticks_symbol": 1}


def grade(sb, elf, g):
    sb.call("load", elf=str(elf), platform="stm32f4")
    sb.call("peripheral_trace", peripheral="usart2")   # enable before any code runs
    sb.call("peripheral_trace", peripheral="timer2")

    # 1. First count line at ~1 s. Stop early otherwise: an interrupt storm
    #    (UIF never cleared) simulates ~30x slower than real firmware.
    out = sb.call("run_until", uart_regex=r"T1 count=1000\r\n", timeout_ms=1100, step_ms=10)
    g.check("first_count", out["hit"], weight=CHECKS["first_count"],
            detail={"elapsed_virtual_ms": out["elapsed_virtual_ms"], "uart": out["text"][-200:]})
    if out["hit"]:
        rest_ms = RUN_S * 1000 - out["virtual_time_s"] * 1000
        sb.call("run_for", virtual_ms=rest_ms)
    t_end = sb.call("serial")["virtual_time_s"]

    # Traces first: register reads below would otherwise appear in them.
    lines = line_times(uart_tx(trace_all(sb, "usart2", op="W", offset=0x4), "stm32f4"))
    tim_w = sb.call("peripheral_trace", peripheral="timer2", op="W", limit=0)["total"]
    tim_r = sb.call("peripheral_trace", peripheral="timer2", op="R", limit=0)["total"]

    # 2. Exactly the expected lines, in order.
    text = [s for _, s in lines]
    g.check("counts", text == LINES, weight=CHECKS["counts"],
            detail={"lines": text[:10], "expected": LINES})

    # 3. Lines one virtual second apart (start -> 1000 -> 2000 -> 3000).
    times = [t for t, _ in lines]
    gaps = [round(b - a, 5) for a, b in zip(times, times[1:]) if a is not None and b is not None]
    ok = text == LINES and len(gaps) == 3 and all(abs(x - 1.0) <= INTERVAL_TOL for x in gaps)
    g.check("interval_1s", ok, weight=CHECKS["interval_1s"],
            detail={"intervals_s": gaps, "tolerance": INTERVAL_TOL})

    # 4. PSC/ARR give exactly 1 kHz with the hardware's formula.
    psc, _ = reg_value(sb, "timer2", "PSC")
    arr, _ = reg_value(sb, "timer2", "ARR")
    hz = TIMER_HZ / ((psc + 1) * (arr + 1))
    g.check("rate_config", hz == 1000, weight=CHECKS["rate_config"],
            detail={"PSC": psc, "ARR": arr, "update_hz_on_hardware": round(hz, 3)})

    # 5. The interrupt drives the count: update interrupt enabled in TIM2 and in
    #    the NVIC (TIM2 is IRQ 28), and nothing polls the timer (reads stay few).
    _, dier = reg_value(sb, "timer2", "DIER")
    iser0 = int.from_bytes(bytes.fromhex(sb.call("mem", addr="0xE000E100", length=4)["hex"]), "little")
    nvic = bool(iser0 & (1 << 28))
    no_poll = tim_r <= 2 * tim_w + 10
    g.check("isr_source", dier.get("UIE") == 1 and nvic and no_poll and tim_w >= 1000,
            weight=CHECKS["isr_source"],
            detail={"DIER.UIE": dier.get("UIE"), "NVIC_TIM2_enabled": nvic,
                    "timer2_writes": tim_w, "timer2_reads": tim_r})

    # 6. TICKS counts milliseconds (same 0.2% allowance as the intervals).
    ticks = int.from_bytes(bytes.fromhex(sb.call("mem", addr="TICKS", length=4)["hex"]), "little")
    want = t_end * 1000
    g.check("ticks_symbol", abs(ticks - want) <= want * INTERVAL_TOL + 2, weight=CHECKS["ticks_symbol"],
            detail={"TICKS": ticks, "expected_ms": round(want, 1)})
