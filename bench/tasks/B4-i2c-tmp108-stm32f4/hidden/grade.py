"""Grader for B4: TMP108 over I2C1 on STM32F4. Hidden from the agent."""

from grading import hexint, last_written, line_times, trace_all, uart_tx

SENSOR = "i2c1:\n    I2CReadCount: 2\n\ntmp108: Sensors.TMP108 @ i2c1 0x48"
TMP = "i2c1.tmp108"
# The simulated TMP108 holds whole degrees; stay clear of its alert range,
# where it sets flag bits in the low byte of the temperature register.
CHECKS = {"first_reading": 2, "follows_sensor": 2, "negative": 1, "cadence_1s": 1,
          "not_found": 1, "i2c_config": 1}


def set_temp(sb, t):
    sb.call("stimulus", peripheral=TMP, property="Temperature", value=t)


def lines_of(text):
    return [l for l in text.split("\r\n") if l]


def grade(sb, elf, g):
    # Phase 1: sensor present.
    sb.call("load", elf=str(elf), platform="stm32f4", extra_repl=SENSOR)
    sb.call("peripheral_trace", peripheral="usart2")
    sb.call("peripheral_trace", peripheral="i2c1")
    set_temp(sb, 23)

    # 1. First reading right after reset.
    out = sb.call("run_until", uart_regex=r"B4 23\.00\r\n", timeout_ms=300, step_ms=10)
    g.check("first_reading", out["hit"], weight=CHECKS["first_reading"],
            detail={"elapsed_virtual_ms": out["elapsed_virtual_ms"], "uart": out["text"][-200:]})

    # 2. The next second's reading follows the sensor.
    set_temp(sb, 41)
    sb.call("run_for", virtual_ms=1100)
    got = lines_of(sb.call("serial")["text"])
    g.check("follows_sensor", "B4 41.00" in got, weight=CHECKS["follows_sensor"],
            detail={"lines": got[:6], "expected": "B4 41.00"})

    # 3. Below zero.
    set_temp(sb, -7)
    sb.call("run_for", virtual_ms=1100)
    got = lines_of(sb.call("serial")["text"])
    g.check("negative", "B4 -7.00" in got, weight=CHECKS["negative"],
            detail={"lines": got[:6], "expected": "B4 -7.00"})

    # 4. One line a second (+-5%). Traces before any register read.
    lines = line_times(uart_tx(trace_all(sb, "usart2", op="W", offset=0x4), "stm32f4"))
    times = [t for t, s in lines if t is not None and s.startswith("B4 ")]
    gaps = [round(b - a, 4) for a, b in zip(times, times[1:])]
    g.check("cadence_1s", len(gaps) >= 2 and all(abs(x - 1.0) <= 0.05 for x in gaps),
            weight=CHECKS["cadence_1s"], detail={"intervals_s": gaps[:6]})

    # 6. I2C1 set up for <= 100 kHz from 16 MHz, and the address bytes 0x90/0x91
    #    (0x48 write/read) on the bus. Values as written: compared with the
    #    hardware formulas, not Renode's (it doesn't model bus timing).
    i2c = sb.call("peripheral_trace", peripheral="i2c1", op="W", limit=100_000)["accesses"]
    last = last_written(i2c)
    freq = last.get(0x04, 0) & 0x3F
    ccr = last.get(0x1C, 0)
    fast, ccr_val = bool(ccr & (1 << 15)), ccr & 0xFFF
    scl = (16e6 / (3 * ccr_val) if fast else 16e6 / (2 * ccr_val)) if ccr_val else 0
    dr = {hexint(a["value"]) & 0xFF for a in i2c if hexint(a["offset"]) == 0x10}
    ok = freq == 16 and 0 < scl <= 100_000 * 1.01 and {0x90, 0x91} <= dr
    g.check("i2c_config", ok, weight=CHECKS["i2c_config"],
            detail={"CR2.FREQ": freq, "CCR": hex(ccr), "scl_hz": round(scl),
                    "address_bytes_seen": sorted(hex(x) for x in dr if x in (0x90, 0x91))})

    # Phase 2 (5): no sensor on the bus.
    sb.call("load", elf=str(elf), platform="stm32f4")
    sb.call("run_for", virtual_ms=1100)
    got = lines_of(sb.call("serial")["text"])
    g.check("not_found", bool(got) and all(l == "B4 sensor not found" for l in got),
            weight=CHECKS["not_found"], detail={"lines": got[:4]})
