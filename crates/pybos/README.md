# pybos

Python bindings for BOS `config`, `bus`, and `agent`.

## Install (dev)

```bash
maturin develop -m crates/pybos/Cargo.toml
```

## API

### `ConfigLoader`

```python
from pybos import ConfigLoader

loader = ConfigLoader(strategy="deep_merge")
loader.add_file("app.toml")
loader.add_inline({"agent": {"model": "gpt-4.1"}})
cfg = loader.load_sync()
```

### `BusConfig` / `Bus`

```python
import asyncio
from pybos import Bus, BusConfig

async def main():
    bus_cfg = BusConfig(mode="peer")
    bus = await Bus.create(bus_cfg)
    await bus.publish_text("demo/topic", "hello from python")

asyncio.run(main())
```

### `Publisher` / `Subscriber`

```python
import asyncio
from pybos import Bus, BusConfig, Publisher, Subscriber

async def main():
    bus = await Bus.create(BusConfig())
    pub = await Publisher.create(bus, "demo/topic")
    sub = await Subscriber.create(bus, "demo/topic")

    await pub.publish_text("hello")
    msg = await sub.recv_with_timeout_ms(1000)
    print(msg)

asyncio.run(main())
```

### `Query` / `Queryable`

```python
import asyncio
from pybos import Bus, BusConfig, Query, Queryable

def uppercase_handler(text: str) -> str:
    return text.upper()

async def main():
    bus = await Bus.create(BusConfig())
    queryable = await Queryable.create(bus, "svc/upper", uppercase_handler)
    await queryable.start()

    query = await Query.create(bus, "svc/upper")
    out = await query.query_text("hello")
    print(out)  # HELLO

asyncio.run(main())
```

### `Caller` / `Callable`

```python
import asyncio
from pybos import Bus, BusConfig, Caller, Callable

def echo_handler(text: str) -> str:
    return f"echo:{text}"

async def main():
    bus = await Bus.create(BusConfig())
    callable_srv = await Callable.create(bus, "svc/echo", echo_handler)
    await callable_srv.start()

    caller = await Caller.create(bus, "svc/echo")
    out = await caller.call_text("ping")
    print(out)  # echo:ping

asyncio.run(main())
```

### `AgentConfig` / `Agent`

```python
import asyncio
from pybos import Agent, AgentConfig

async def main():
    cfg = AgentConfig(
        name="assistant",
        model="gpt-4.1",
        api_key="sk-...",
        base_url="https://api.openai.com/v1",
    )
    agent = Agent.from_config(cfg)
    text = await agent.run("Say hello in one sentence")
    print(text)

asyncio.run(main())
```

## Best practices

- Use `asyncio.run(...)` at process entry and `await` all async API calls.
- Keep `AgentConfig` immutable after creation for reproducibility.
- Prefer `ConfigLoader` + inline overrides for environment-specific setup.
- Reuse one `Bus` instance per process, not per call.
- Keep query/call handlers fast and side-effect-light; move heavy IO to async tasks.
