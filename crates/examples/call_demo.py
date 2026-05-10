import asyncio
from pybrainos import Bus, BusConfig, Callable, Caller

async def main():
    bus = await Bus.create(BusConfig())

    def add(req):
        a, b = map(int, req.split(","))
        return str(a + b)

    rpc = await Callable.create(bus, "rpc/add")

    async def run_rpc():
        try:
            await rpc.run(add)
        except asyncio.CancelledError:
            pass

    task = asyncio.create_task(run_rpc())

    await asyncio.sleep(0.2)
    caller = await Caller.create(bus, "rpc/add")
    print(await caller.call_text("5,7"))  # 12

    task.cancel()
    await task

if __name__ == "__main__":
    asyncio.run(main())
