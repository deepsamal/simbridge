"""Drive simbridge over real MCP stdio, the same way Claude Code would."""
import asyncio, json, sys
from pathlib import Path
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

ROOT = Path(__file__).resolve().parents[1]
ELF = ROOT / "firmware/stm32f4-hello/target/thumbv7em-none-eabihf/release/stm32f4-hello"

async def main():
    params = StdioServerParameters(command=sys.executable, args=["-m", "simbridge"], cwd=str(ROOT))
    async with stdio_client(params) as (rd, wr):
        async with ClientSession(rd, wr) as s:
            await s.initialize()
            print("tools:", [t.name for t in (await s.list_tools()).tools])
            async def call(name, **kw):
                res = await s.call_tool(name, kw)
                d = json.loads(res.content[0].text)
                print("%-17s %s" % (name, json.dumps(d)[:240]))
                assert d.get("ok"), d
                return d
            await call("load", elf=str(ELF), platform="stm32f4")
            await call("peripheral_trace", peripheral="usart2")
            await call("run_for", virtual_ms=30)
            out = await call("serial")
            assert "hello from stm32f4" in out["text"]
            await call("peripheral_trace", peripheral="usart2", limit=2)
            await call("regs", peripheral="usart2", count=4)
            a = await call("mem", addr="TICKS", length=4)
            snap = await call("snapshot")
            await call("run_for", virtual_ms=30)
            await call("restore", id=snap["id"])
            b = await call("mem", addr="TICKS", length=4)
            assert a["hex"] == b["hex"]
            await call("reset")
    print("MCP OK")

asyncio.run(main())
