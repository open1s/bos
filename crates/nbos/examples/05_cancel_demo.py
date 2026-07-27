#!/usr/bin/env python3
"""
Agent Cancel Demo (nbos) — Real cancellation with two scenarios.

Scenario A: External bus cancel — a separate watcher subscribes to
  lifecycle events and publishes cancel on the bus after observing
  slow_op start. This mimics an operator / control-plane cancelling a
  long-running agent tool externally.

Scenario B: Controller subscriber — a "controller" watches the worker
  agent's tool events, and when it sees slow_op running, it publishes
  cancel. This shows cross-component coordination where a policy engine
  or human operator manages agent execution.

Both scenarios test the full path:
  bus publish -> engine ToolRunMgr -> tool.cancel(call_id) ->
  PyPythonTool.cancel_callback(call_id) -> Python cancel handler ->
  threading.Event.set() -> slow_op poll loop exits.

Run with:
    python crates/nbos/examples/05_cancel_demo.py
    RUST_LOG=debug python crates/nbos/examples/05_cancel_demo.py
"""

import asyncio
import json
import threading
import time

from nbos import (
    BrainOS,
    init_tracing,
    ConfigLoader,
    Publisher,
    Subscriber,
    PythonTool,
)


# ── Shared slow_op tool factory ────────────────────────────────────────────

def create_slow_op():
    """Returns (callback_fn, cancel_fn) pair sharing a per-call_id Event dict."""
    events: dict[str, threading.Event] = {}

    def callback(args: dict) -> str:
        key = args.get("key", "unknown")
        call_id = args.get("__call_id__", "unknown")
        done = threading.Event()
        events[call_id] = done

        print(f"  [slow_op] start  call_id={call_id}  key={key}")
        start = time.monotonic()

        result = "done"
        for _ in range(70):  # ~7s at 100ms per tick
            if done.wait(0.1):
                result = "cancelled"
                break
        else:
            done.set()

        elapsed = time.monotonic() - start
        print(f"  [slow_op] end    call_id={call_id}  result={result}  ({elapsed:.1f}s)")
        events.pop(call_id, None)
        return json.dumps({"key": key, "call_id": call_id, "result": result})

    def cancel(call_id: str) -> None:
        print(f"  [slow_op] cancel call_id={call_id}")
        event = events.get(call_id)
        if event:
            event.set()
        else:
            print(f"  [slow_op] no active event for call_id={call_id}")

    return callback, cancel


SLOW_OP_SCHEMA = {
    "type": "object",
    "properties": {
        "key": {"type": "string", "description": "A key to process slowly (~7s)"},
    },
    "required": ["key"],
}


# ── Helpers ────────────────────────────────────────────────────────────────

def build_tool(name, description, schema, callback, cancel_callback):
    t = PythonTool(
        name=name,
        description=description,
        parameters=json.dumps(schema["properties"]),
        schema=json.dumps(schema),
        callback=callback,
    )
    t.cancelable()
    t.set_cancel_callback(cancel_callback)
    return t


# ── Scenario A: External watcher cancels a running tool ────────────────────

async def scenario_a_external_watcher(brain, model):
    print("")
    print("=" * 60)
    print("  Scenario A: External bus-watcher cancels the agent")
    print("=" * 60)
    print("")

    bus = brain.bus
    agent_name = "cancel-demo-a"
    cancel_topic = f"agent/{agent_name}/tool/cancel"
    events_topic = f"agent/{agent_name}/tool/events"
    print(f"[bus] cancel topic:  {cancel_topic}")
    print(f"[bus] events topic:  {events_topic}")

    agent = await (
        brain.agent(
            agent_name,
            model=model,
            system_prompt=(
                "You are a test assistant. Use the slow_op tool exactly once "
                'with key "scenario-A", then tell me the result.'
            ),
        )
        .start()
    )
    print("[ok] agent started")

    callback, cancel_cb = create_slow_op()
    tool = build_tool("slow_op", "A slow ~7s operation. Cancelable.", SLOW_OP_SCHEMA, callback, cancel_cb)
    await agent._inner.add_tool(tool)
    print("[ok] slow_op registered as cancelable\n")

    events_sub = await Subscriber.create(bus, events_topic)
    cancel_pub = await Publisher.create(bus, cancel_topic)

    started_id = None
    started_event = asyncio.Event()

    async def collect_events():
        nonlocal started_id
        while True:
            event = await events_sub.recv_json_with_timeout_ms(500)
            if event is None:
                continue
            s = event if isinstance(event, dict) else {}
            print(f"  [watcher] {s.get('status')}: {s.get('tool')} (call_id={s.get('call_id')})")
            if s.get("status") == "started" and not started_id:
                started_id = s.get("call_id")
                started_event.set()

    events_task = asyncio.create_task(collect_events())

    ask_task = asyncio.create_task(
        agent.ask('Use slow_op with key "scenario-A" and summarize the result.')
    )

    # Wait for slow_op to start (up to 60s for LLM), then publish cancel.
    try:
        await asyncio.wait_for(started_event.wait(), timeout=60.0)
        print(f"\n[watcher] Publishing cancel: call_id={started_id}")
        await cancel_pub.publish_json({"call_id": started_id})
    except asyncio.TimeoutError:
        print("\n[watcher] Timed out waiting for slow_op to start — may complete on its own\n")

    try:
        result = await ask_task
        print("\n--- Agent final response ---")
        print(result)
    except Exception as e:
        print(f"\n--- Agent errored: {type(e).__name__}: {e} ---")

    events_task.cancel()
    try:
        await events_task
    except asyncio.CancelledError:
        pass

    await events_sub.stop()

    print("\n" + "-" * 40)
    print("Scenario A complete.")
    print("Path: events sub -> external bus cancel publish ->")
    print("      engine ToolRunMgr -> tool.cancel(call_id) -> Event.set()")
    print("-" * 40)


# ── Scenario B: Controller subscriber cancels worker agent ─────────────────

async def scenario_b_controller_worker(brain, model):
    print("")
    print("=" * 60)
    print("  Scenario B: Controller cancels worker agent")
    print("=" * 60)
    print("")

    bus = brain.bus

    # ── Worker agent ──
    worker_name = "worker-agent"
    cancel_topic = f"agent/{worker_name}/tool/cancel"
    events_topic = f"agent/{worker_name}/tool/events"
    print(f"[bus] worker cancel:  {cancel_topic}")
    print(f"[bus] worker events:  {events_topic}\n")

    worker = await (
        brain.agent(
            worker_name,
            model=model,
            system_prompt=(
                "You are a worker. Use the slow_op tool exactly once "
                'with key "scenario-B", then report the result.'
            ),
        )
        .start()
    )
    print("[ok] worker started")

    callback, cancel_cb = create_slow_op()
    tool = build_tool("slow_op", "A slow ~7s operation. Cancelable.", SLOW_OP_SCHEMA, callback, cancel_cb)
    await worker._inner.add_tool(tool)
    print("[ok] worker slow_op registered\n")

    # ── Controller subscriber ──
    # In a real system this would be a policy engine, anomaly detector,
    # or human operator. Here it watches events and cancels after 3s.
    events_sub = await Subscriber.create(bus, events_topic)
    cancel_pub = await Publisher.create(bus, cancel_topic)

    started_id = None
    started_event = asyncio.Event()

    async def watch_events():
        nonlocal started_id
        while True:
            event = await events_sub.recv_json_with_timeout_ms(500)
            if event is None:
                continue
            s = event if isinstance(event, dict) else {}
            print(f"  [controller] event: {s.get('status')} {s.get('tool')} (call_id={s.get('call_id')})")
            if s.get("status") == "started" and not started_id:
                started_id = s.get("call_id")
                started_event.set()

    events_task = asyncio.create_task(watch_events())

    worker_task = asyncio.create_task(
        worker.ask('Use slow_op with key "scenario-B" and summarize the result.')
    )

    print("[controller] Waiting for worker to start slow_op...\n")
    try:
        await asyncio.wait_for(started_event.wait(), timeout=60.0)
        print(f"\n[controller] Cancelling worker: call_id={started_id}")
        print("[controller] Reason: processing budget exceeded for scenario-B")
        await cancel_pub.publish_json({"call_id": started_id})
    except asyncio.TimeoutError:
        print("\n[controller] Timed out waiting for slow_op to start — skip cancel\n")

    try:
        result = await worker_task
        print("\n--- Worker final response ---")
        print(result)
    except Exception as e:
        print(f"\n--- Worker errored: {type(e).__name__}: {e} ---")

    events_task.cancel()
    try:
        await events_task
    except asyncio.CancelledError:
        pass

    await events_sub.stop()

    print("\n" + "-" * 40)
    print("Scenario B complete.")
    print("  Controller watches worker events,")
    print("  decides to cancel based on policy (time budget),")
    print("  publishes cancel -> slow_op loop exits immediately.")
    print("-" * 40)


# ── Main ───────────────────────────────────────────────────────────────────

async def main():
    print("")
    print("=" * 60)
    print("   BrainOS Agent Cancel Demo (nbos) — Two Scenarios")
    print("=" * 60)

    loader = ConfigLoader()
    loader.discover()
    config = loader.load_sync()
    global_model = config.get("global_model", {})
    model = global_model.get("model")

    init_tracing()

    api_key = global_model.get("api_key", "")
    if not api_key:
        print("[SKIP] No API key; set it in ~/.bos/conf/config.toml")
        return

    print(f"Model: {model}")

    async with BrainOS() as brain:
        await scenario_a_external_watcher(brain, model)
        await scenario_b_controller_worker(brain, model)

    print("\nAll cancel demos finished.\n")


if __name__ == "__main__":
    asyncio.run(main())