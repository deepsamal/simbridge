"""Grader for B3: UART0 banner on FE310. Hidden from the agent."""

from grading import hexint, last_written

BANNER = b"B3 ready\r\n"
UART0 = 0x1001_3000
CHECKS = {"banner": 2, "banner_once": 1, "txdata_writes": 1, "fifo_full_polled": 1,
          "txen": 1, "baud_div": 1}


def grade(sb, elf, g):
    sb.call("load", elf=str(elf), platform="fe310")
    sb.call("peripheral_trace", peripheral="uart0")  # enable before any code runs

    # 1. The banner appears within 100 ms of virtual time.
    out = sb.call("run_until", uart_regex=r"B3 ready\r\n", timeout_ms=100)
    g.check("banner", out["hit"], weight=CHECKS["banner"],
            detail={"elapsed_virtual_ms": out["elapsed_virtual_ms"], "uart": out["text"][-200:]})

    # 2. Nothing else in the next 100 ms.
    sb.call("run_for", virtual_ms=100)
    rest = sb.call("serial")["text"]
    g.check("banner_once", out["hit"] and rest == "", detail={"after_banner": rest[:200]})

    # Every UART0 access, in order (a few dozen for a correct firmware).
    every = sb.call("peripheral_trace", peripheral="uart0", limit=100_000)["accesses"]
    acc = [a for a in every if hexint(a["offset"]) == 0x0]  # txdata

    # 3. The text really went through txdata, byte for byte.
    sent = bytes(hexint(a["value"]) & 0xFF for a in acc if a["op"] == "W")
    g.check("txdata_writes", sent == BANNER, weight=CHECKS["txdata_writes"],
            detail={"txdata_bytes": sent.decode("ascii", "replace")[:200]})

    # 4. txdata.full read before every write. (Renode's FIFO never fills, so
    #    firmware that skips the check works here and drops bytes on silicon.)
    unpolled, polled = 0, False
    for a in acc:
        if a["op"] == "R":
            polled = True
        else:
            unpolled += not polled
            polled = False
    g.check("fifo_full_polled", bool(sent) and unpolled == 0, weight=CHECKS["fifo_full_polled"],
            detail={"writes_without_a_preceding_read": unpolled})

    # 5. Transmitter enabled, and div for 115200 from 16 MHz: f / (div + 1).
    #    From the last value written: Renode's UART model doesn't keep div.
    last = last_written(every)
    txctrl, div = last.get(0x08, 0), last.get(0x18)
    g.check("txen", txctrl & 1 == 1, weight=CHECKS["txen"], detail={"txctrl": hex(txctrl)})
    baud = 16_000_000 / (div + 1) if div is not None else 0
    g.check("baud_div", abs(baud - 115_200) / 115_200 < 0.01, weight=CHECKS["baud_div"],
            detail={"div": div, "baud": round(baud), "expected_div": 138})
