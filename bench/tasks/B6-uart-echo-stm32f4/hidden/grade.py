"""Grader for B6: upper-case UART echo on STM32F4. Hidden from the agent."""

from grading import last_written

# 20 lines the spec doesn't show: mixed case, digits and punctuation, empty
# lines, CRLF and bare LF endings, non-letters that must not change, and a
# 128-byte line.
LONG = "".join(chr(ord("a") + i % 26) if i % 9 else "-" for i in range(128))
FIRST = ["Hello, World!", "", "mIxEd CaSe 123"]
BURST = ["the quick brown fox", "", "", "~`!@#$%^&*()_+{}|:<>?", "already UPPER",
         "z", "a1b2c3d4e5", "tab\tin\tline", "x" * 127, LONG, "umlaut ue: ü",
         "  spaces  ", "end with cr only?", "last but two", "last but one", "last"]
CHECKS = {"ready": 1, "echo_basic": 2, "empty_lines": 1, "long_line": 1, "burst": 1,
          "rx_enabled": 1}


def upper(s):
    return "".join(chr(ord(c) - 32) if "a" <= c <= "z" else c for c in s)


def send(sb, lines, crlf):
    data = "".join(l + ("\r\n" if (i % 2 == 0) == crlf else "\n") for i, l in enumerate(lines))
    sb.call("uart_write", hex=data.encode("utf-8").hex())


def grade(sb, elf, g):
    sb.call("load", elf=str(elf), platform="stm32f4")
    sb.call("peripheral_trace", peripheral="usart2")

    out = sb.call("run_until", uart_regex=r"B6 ready\r\n", timeout_ms=100)
    g.check("ready", out["hit"], weight=CHECKS["ready"], detail={"uart": out["text"][-200:]})

    # 6. Receiver enabled in CR1 (RE, bit 2), as written during start-up. Then
    #    stop tracing: a firmware waiting on RXNE polls SR constantly, and
    #    logging every poll would slow the run down tenfold.
    tr = sb.call("peripheral_trace", peripheral="usart2", op="W", limit=10_000)["accesses"]
    cr1 = last_written(tr).get(0x0C, 0)
    g.check("rx_enabled", bool(cr1 & (1 << 2)) and bool(cr1 & (1 << 13)),
            weight=CHECKS["rx_enabled"], detail={"CR1": hex(cr1)})
    sb.call("peripheral_trace", peripheral="usart2", disable=True)

    # 1-2. A few lines, one write each, with the machine running in between.
    got = ""
    for line in FIRST:
        send(sb, [line], crlf=True)
        sb.call("run_for", virtual_ms=3)
        got += sb.call("serial", max_bytes=65536)["text"]
    want = "".join(upper(l) + "\r\n" for l in FIRST)
    g.check("echo_basic", got == want, weight=CHECKS["echo_basic"],
            detail={"got": got[:300], "want": want})

    # 3-5. Sixteen lines back to back in one write (CRLF and LF mixed).
    send(sb, BURST, crlf=False)
    sb.call("run_for", virtual_ms=250)  # Renode feeds received bytes in at ~0.3 ms each
    got = sb.call("serial", max_bytes=65536)["text"]
    got_lines = got.split("\r\n")[:-1] if got.endswith("\r\n") else got.split("\r\n")
    want_lines = [upper(l) for l in BURST]
    g.check("empty_lines", got_lines[1:3] == ["", ""] and got.startswith(want_lines[0] + "\r\n"),
            weight=CHECKS["empty_lines"], detail={"first_lines": got_lines[:4]})
    long_ok = want_lines[9] in got_lines and want_lines[8] in got_lines
    g.check("long_line", long_ok, weight=CHECKS["long_line"],
            detail={"want": want_lines[9], "got_lengths": [len(l) for l in got_lines][:16]})
    g.check("burst", got_lines == want_lines, weight=CHECKS["burst"],
            detail={"got": got_lines[:16], "want": want_lines})

