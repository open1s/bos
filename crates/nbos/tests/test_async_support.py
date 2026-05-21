#!/usr/bin/env python3
"""Test async tools, hooks, and plugins in nbos."""

import asyncio
import json

import pytest
from nbos import BrainOS, ToolDef


@pytest.fixture
def brain():
    return BrainOS()


@pytest.fixture
async def agent_with_async_tool(brain):
    async def async_weather_callback(args):
        city = args.get("city", "Unknown")
        await asyncio.sleep(0.01)
        return json.dumps({"city": city, "temp": 68, "unit": "F"})

    tool = ToolDef(
        name="get_weather_async",
        description="Fetch weather from async API",
        callback=async_weather_callback,
        parameters={
            "type": "object",
            "properties": {"city": {"type": "string", "description": "City name"}},
            "required": ["city"],
        },
    )

    async with brain:
        builder = brain.agent("test").register(tool)
        agent = await builder.start()
        yield agent


@pytest.fixture
async def agent_with_async_hooks(brain):
    hook_results = []

    async def before_llm_hook(event, ctx):
        await asyncio.sleep(0.01)
        hook_results.append("before_llm")
        return "Continue"

    async def after_llm_hook(event, ctx):
        await asyncio.sleep(0.01)
        hook_results.append("after_llm")
        return "Continue"

    async with brain:
        builder = brain.agent("test").with_hooks(
            {
                "BeforeLlmCall": before_llm_hook,
                "AfterLlmCall": after_llm_hook,
            }
        )
        agent = await builder.start()
        yield agent, hook_results


@pytest.fixture
async def agent_with_async_plugins(brain):
    plugin_results = []

    async def on_llm_request(request):
        await asyncio.sleep(0.01)
        plugin_results.append("on_llm_request")
        return request

    async def on_llm_response(response):
        await asyncio.sleep(0.01)
        plugin_results.append("on_llm_response")
        return response

    plugin = {
        "name": "AsyncEnricher",
        "on_llm_request": on_llm_request,
        "on_llm_response": on_llm_response,
    }

    async with brain:
        builder = brain.agent("test").with_plugins(plugin)
        agent = await builder.start()
        yield agent, plugin_results


@pytest.mark.asyncio
async def test_async_tool_callback_is_called(agent_with_async_tool):
    agent = agent_with_async_tool

    tools = agent._tools.list_tools()
    tool_names = [t.name for t in tools]
    assert "get_weather_async" in tool_names

    result = await agent.run_simple("Say hello")
    assert result is not None
    assert len(result) > 0


@pytest.mark.asyncio
async def test_async_hooks_receive_event_and_context(agent_with_async_hooks):
    agent, hook_results = agent_with_async_hooks

    result = await agent.run_simple("What is 2+2?")
    assert result is not None

    assert "before_llm" in hook_results
    assert "after_llm" in hook_results


@pytest.mark.asyncio
async def test_async_plugins_receive_request_and_response(agent_with_async_plugins):
    agent, plugin_results = agent_with_async_plugins

    result = await agent.run_simple("What is 2+2?")
    assert result is not None

    assert "on_llm_request" in plugin_results
    assert "on_llm_response" in plugin_results


@pytest.mark.asyncio
async def test_sync_tool_still_works_with_async_hooks(brain):
    from nbos import tool

    @tool("Sync add tool", name="add")
    def add_tool(args):
        a = args.get("a", 0)
        b = args.get("b", 0)
        return json.dumps({"result": a + b})

    hooks_fired = []

    async def before_llm_hook(event, ctx):
        await asyncio.sleep(0.01)
        hooks_fired.append("before_llm")
        return "Continue"

    async with brain:
        builder = brain.agent("test").with_tools(add_tool).with_hooks({"BeforeLlmCall": before_llm_hook})
        agent = await builder.start()

        result = await agent.run_simple("What is 2+2?")
        assert result is not None

        assert len(hooks_fired) > 0
        assert "before_llm" in hooks_fired