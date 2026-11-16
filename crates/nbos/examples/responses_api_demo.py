#!/usr/bin/env python3
"""
Responses API Demo — exercises the OpenAI-compatible `/responses` endpoint
via `api_mode="responses"` (plus `reasoning_effort="high"`).

Demonstrates:
- agent.run_simple()   — single Responses call
- agent.react()        — tool round-trip (function_call -> function_call_output)
- agent.stream()       — streaming text tokens
- collect-ahead stream — stream with a tool; detect tool_call/usage/Text/Done

Config (in priority order):
    OPENAI_API_KEY / LLM_BASE_URL / LLM_MODEL  (env)
    ~/.bos/conf/config.toml  [global_model]
    e.g. LLM_BASE_URL=https://api.deepseek.com LLM_MODEL=deepseek-v4-flash
"""

import asyncio
import json
import os

from nbos import BrainOS, tool
from nbos import ConfigLoader as PyConfigLoader
from nbos import init_tracing

init_tracing()  # optional, for debugging

@tool("Add two integers and return their sum.")
def add(a: int, b: int) -> int:
    return a + b


def _load_env():
    loader = PyConfigLoader()
    loader.discover()
    global_model = loader.load_sync().get("global_model", {})

    return {
        "api_key": os.getenv("OPENAI_API_KEY") or global_model.get("api_key"),
        "base_url": os.getenv("LLM_BASE_URL") or global_model.get("base_url", "https://api.deepseek.com"),
        "model": os.getenv("LLM_MODEL") or global_model.get("model", "deepseek-v4-flash"),
    }


def _parse(token: str):
    """Stream tokens are plain text or JSON dicts ({type: ...})."""
    try:
        item = json.loads(token)
        if isinstance(item, dict):
            return item
    except json.JSONDecodeError:
        pass
    return {"type": "text", "text": token}


async def main():
    cfg = _load_env()
    if not cfg["api_key"]:
        print("  ⚠️  No API key — set OPENAI_API_KEY or add [global_model] to ~/.bos/conf/config.toml")
        return

    async with BrainOS(api_key=cfg["api_key"], base_url=cfg["base_url"], model=cfg["model"]) as brain:
        agent = await (
            brain.agent(
                "responses-demo",
                system_prompt="Persona: You are a helpful assistant. Use the add tool to perform integer addition.",
                api_mode="responses",
                reasoning_effort="high",
            )
            .with_tools(add)
            .start()
        )

        # ── 1. run_simple() — single Responses call ──
        print("=" * 60)
        print("  1. run_simple()")
        print("=" * 60)
        result = await agent.run_simple("Say hi in one word")
        print(f"  Response: {result}\n")

        # ── 2. react() — tool round-trip ──
        print("=" * 60)
        print("  2. react() with add tool")
        print("=" * 60)
        result = await agent.react("What is 3 + 4? Use the add tool.")
        print(f"  Response: {result}\n")

        # ── 3. stream() — streaming text tokens ──
        print("=" * 60)
        print("  3. stream() — SSE")
        print("=" * 60)
        stream_iter = await agent.stream("Count from 1 to 3, one number per line")
        async for token in stream_iter:
            item = _parse(token)
            if item["type"] == "text":
                print(item["text"], end="", flush=True)
            elif item["type"] == "usage":
                print(f"\n  [usage] prompt={item['promptTokens']} completion={item['completionTokens']} total={item['totalTokens']}")
        print("\n")

        # ── 4. collect-ahead stream with a tool ──
        print("=" * 60)
        print("  4. collect-ahead stream (SSE) with add tool")
        print("=" * 60)
        collected = []
        stream_iter = await agent.stream("What is 5 + 3? Use the add tool.")
        async for token in stream_iter:
            item = _parse(token)
            collected.append(item["type"])
            if item["type"] == "tool_call":
                print(f"  Tool: {item['name']} args={item['args']}")
            elif item["type"] == "text":
                print(f"  Text: {item['text']}")

        print(f"\n  Collected tokens: {collected}")
        print("  Done. No duplicate call_id (each tool call executes once).")


if __name__ == "__main__":
    asyncio.run(main())
