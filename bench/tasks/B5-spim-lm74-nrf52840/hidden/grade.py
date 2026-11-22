"""Grader for B5: LM74 over SPIM2 on nRF52840. Hidden from the agent."""

from grading import hexint, last_written

SENSOR = "spi2:\n    easyDMA: true\n\nlm74: Sensors.TI_LM74 @ spi2"
LM74 = "spi2.lm74"
RAM = (0x2000_0000, 0x2004_0000)
CS = 20  # P0.20
CHECKS = {"first_reading": 2, "follows_sensor": 2, "negative": 1, "cadence_1s": 1,
          "spim_easydma": 1, "cs_framing": 1}


def set_temp(sb, t):
    sb.call("stimulus", peripheral=LM74, property="Temperature", value=t)


def lines_of(text):
    return [l for l in text.split("\r\n") if l]


def grade(sb, elf, g):
    sb.call("load", elf=str(elf), platform="nrf52840", extra_repl=SENSOR)
    sb.call("peripheral_trace", peripheral="spi2")
    sb.call("peripheral_trace", peripheral="gpio0")
    set_temp(sb, 23.5)

    # 1. First reading right after reset.
    out = sb.call("run_until", uart_regex=r"B5 23\.50\r\n", timeout_ms=300, step_ms=10)
    g.check("first_reading", out["hit"], weight=CHECKS["first_reading"],
            detail={"elapsed_virtual_ms": out["elapsed_virtual_ms"], "uart": out["text"][-200:]})
    t0 = out["virtual_time_s"]

    # 2. The next second's reading follows the sensor (a fraction the spec's
    #    examples don't use).
    set_temp(sb, 41.25)
    sb.call("run_for", virtual_ms=1100)
    s = sb.call("serial")
    got = lines_of(s["text"])
    g.check("follows_sensor", "B5 41.25" in got, weight=CHECKS["follows_sensor"],
            detail={"lines": got[:6], "expected": "B5 41.25"})

    # 3. Below zero.
    set_temp(sb, -7.75)
    sb.call("run_for", virtual_ms=1100)
    got2 = lines_of(sb.call("serial")["text"])
    g.check("negative", "B5 -7.75" in got2, weight=CHECKS["negative"],
            detail={"lines": got2[:6], "expected": "B5 -7.75"})

    # Traces before any register read: SPIM2 and the CS pin.
    spi = sb.call("peripheral_trace", peripheral="spi2", op="W", limit=100_000)["accesses"]
    gpio = sb.call("peripheral_trace", peripheral="gpio0", op="W", limit=100_000)["accesses"]
    starts = [a for a in spi if hexint(a["offset"]) == 0x010 and hexint(a["value"]) == 1]

    # 4. One reading a second: SPIM transfers started 1 s (+-5%) apart.
    times = [a["t"] for a in starts if "t" in a]
    gaps = [round(b - a, 4) for a, b in zip(times, times[1:])]
    g.check("cadence_1s", len(gaps) >= 2 and all(abs(x - 1.0) <= 0.05 for x in gaps),
            weight=CHECKS["cadence_1s"], detail={"transfer_intervals_s": gaps[:6]})

    # 5. Through SPIM's EasyDMA: RXD.PTR into RAM, TASKS_START, ENABLE = 7 (SPIM).
    last = last_written(spi)
    rx_ptrs = [hexint(a["value"]) for a in spi if hexint(a["offset"]) == 0x534]
    ok = (last.get(0x500) == 7 and bool(starts) and bool(rx_ptrs)
          and all(RAM[0] <= p < RAM[1] for p in rx_ptrs) and last.get(0x538, 0) >= 2)
    g.check("spim_easydma", ok, weight=CHECKS["spim_easydma"],
            detail={"ENABLE": last.get(0x500), "RXD.PTR": [hex(p) for p in rx_ptrs][:4],
                    "RXD.MAXCNT": last.get(0x538), "transfers": len(starts)})

    # 6. CS (P0.20) is an output, driven low for each transfer and high again.
    #    Renode's gpio0 sits at 0x50000500: OUT 0x004, OUTSET 0x008,
    #    OUTCLR 0x00C, DIR 0x014, DIRSET 0x018, PIN_CNF[n] 0x200 + 4n.
    bit = 1 << CS
    lows = highs = 0
    is_output = False
    for a in gpio:
        off, v = hexint(a["offset"]), hexint(a["value"])
        if off == 0x00C and v & bit:
            lows += 1
        elif off == 0x008 and v & bit:
            highs += 1
        elif off == 0x004:
            lows, highs = (lows + 1, highs) if not v & bit else (lows, highs + 1)
        if (off in (0x014, 0x018) and v & bit) or (off == 0x200 + 4 * CS and v & 1):
            is_output = True
    ok = is_output and lows >= len(starts) >= 2 and highs >= len(starts)
    g.check("cs_framing", ok, weight=CHECKS["cs_framing"],
            detail={"cs_output": is_output, "cs_low": lows, "cs_high": highs,
                    "transfers": len(starts)})
