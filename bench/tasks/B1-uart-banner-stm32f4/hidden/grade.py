"""Grader for B1: USART2 banner on STM32F4. Hidden from the agent."""

from grading import hexint, reg_value, trace_all

BANNER = b"B1 ready\r\n"
# Every check this grader records, with its weight; any not reached are marked failed.
CHECKS = {"banner": 2, "banner_once": 1, "dr_writes": 1, "brr_115200": 1, "cr1_ue_te": 1}


def grade(sb, elf, g):
    sb.call("load", elf=str(elf), platform="stm32f4")
    sb.call("peripheral_trace", peripheral="usart2")  # start tracing before reset code runs

    # 1. The banner appears within 100 ms of virtual time.
    out = sb.call("run_until", uart_regex=r"B1 ready\r\n", timeout_ms=100)
    g.check("banner", out["hit"], weight=CHECKS["banner"],
            detail={"elapsed_virtual_ms": out["elapsed_virtual_ms"], "uart": out["text"][-200:]})

    # 2. Nothing else is printed in the next 100 ms (once, then idle).
    sb.call("run_for", virtual_ms=100)
    rest = sb.call("serial")["text"]
    g.check("banner_once", out["hit"] and rest == "", detail={"after_banner": rest[:200]})

    # 3. The text really went through USART2's data register, byte for byte.
    #    (Read the trace before regs() so model reads never mix into it.)
    writes = trace_all(sb, "usart2", op="W", offset=0x4)
    sent = bytes(hexint(a["value"]) & 0xFF for a in writes)
    g.check("dr_writes", sent == BANNER, detail={"dr_bytes": sent.decode("ascii", "replace")[:200]})

    # 4. Register configuration, decoded through the SVD.
    brr, _ = reg_value(sb, "usart2", "BRR")
    g.check("brr_115200", brr == 0x8B, detail={"BRR": hex(brr), "expected": "0x8b"})
    cr1, fields = reg_value(sb, "usart2", "CR1")
    g.check("cr1_ue_te", fields.get("UE") == 1 and fields.get("TE") == 1,
            detail={"CR1": hex(cr1), "UE": fields.get("UE"), "TE": fields.get("TE")})
