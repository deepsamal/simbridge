"""Grader for B2: UARTE0 banner through EasyDMA on nRF52840. Hidden from the agent."""

from grading import count, hexint, last_written

BANNER = b"B2 ready\r\n"
UARTE0 = 0x4000_2000
RAM = (0x2000_0000, 0x2004_0000)  # 256 KiB: EasyDMA can only read RAM
CHECKS = {"banner": 2, "banner_once": 1, "easydma_tx": 1, "uarte_config": 1}


def grade(sb, elf, g):
    sb.call("load", elf=str(elf), platform="nrf52840")
    sb.call("peripheral_trace", peripheral="uart0")  # enable before any code runs

    # 1. The banner appears within 100 ms of virtual time.
    out = sb.call("run_until", uart_regex=r"B2 ready\r\n", timeout_ms=100)
    g.check("banner", out["hit"], weight=CHECKS["banner"],
            detail={"elapsed_virtual_ms": out["elapsed_virtual_ms"], "uart": out["text"][-200:]})

    # 2. Nothing else in the next 100 ms.
    sb.call("run_for", virtual_ms=100)
    rest = sb.call("serial")["text"]
    g.check("banner_once", out["hit"] and rest == "", detail={"after_banner": rest[:200]})

    # 3. Sent with EasyDMA from RAM: TXD.PTR into RAM, TASKS_STARTTX, and never
    #    the legacy UART's TXD register (0x51C). Traces before any register read.
    tr = sb.call("peripheral_trace", peripheral="uart0", op="W", limit=10_000)
    writes = [(hexint(a["offset"]), hexint(a["value"])) for a in tr["accesses"]]
    ptrs = [v for o, v in writes if o == 0x544]
    started = any(o == 0x008 and v == 1 for o, v in writes)
    legacy = count(tr["counts"], "W", 0x51C)
    ok = bool(ptrs) and all(RAM[0] <= p < RAM[1] for p in ptrs) and started and legacy == 0
    g.check("easydma_tx", ok, weight=CHECKS["easydma_tx"],
            detail={"TXD.PTR": [hex(p) for p in ptrs][:8], "TASKS_STARTTX": started,
                    "legacy_TXD_writes": legacy})

    # 4. Configuration a real nRF52840 needs (Renode transmits without it).
    #    From the last value written: Renode's UARTE doesn't keep these.
    last = last_written(tr["accesses"])
    enable, baud, psel = last.get(0x500), last.get(0x524), last.get(0x50C)
    g.check("uarte_config", enable == 8 and baud == 0x01D7_E000 and psel == 6,
            weight=CHECKS["uarte_config"],
            detail={"ENABLE": enable, "BAUDRATE": baud and hex(baud), "PSEL.TXD": psel and hex(psel),
                    "expected": {"ENABLE": 8, "BAUDRATE": "0x1d7e000", "PSEL.TXD": "0x6"}})
