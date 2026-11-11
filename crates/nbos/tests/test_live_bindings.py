"""Live tool/skill and compaction bindings."""

import json

import pytest

from nbos_native import Agent as NativeAgent
from nbos_native import AgentConfig, PythonTool


def make_agent() -> NativeAgent:
    config = AgentConfig()
    config.name = "live-test"
    config.model = "openai/gpt-4o"
    config.base_url = "http://127.0.0.1:9/v1"
    config.api_key = "test"
    config.system_prompt = "test"
    config.temperature = 0
    config.timeout_secs = 5
    config.api_mode = "chat"
    return NativeAgent.from_config(config)


def test_skill_metadata_reports_version_and_tags():
    agent = make_agent()
    agent.register_skill("reviewer", "Review code", "Read the diff first.")
    reviewer = next(skill for skill in agent.list_skills() if skill["name"] == "reviewer")
    assert reviewer["version"] == "1.0.0"
    assert reviewer["tags"] == []


def test_live_skills_are_visible_without_restarting_the_agent():
    agent = make_agent()
    agent.add_live_skill("live", "Live skill", "Act now.")
    assert any(skill["name"] == "live" for skill in agent.live_skills())
    assert any(skill["name"] == "live" for skill in agent.list_skills())

    assert agent.remove_live_skills_by_paths([".bos/skills/live"]) == 1
    assert all(skill["name"] != "live" for skill in agent.live_skills())


@pytest.mark.asyncio
async def test_live_tool_and_full_tool_definitions():
    agent = make_agent()
    tool = PythonTool(
        name="lookup",
        description="Look up a record",
        parameters="{}",
        schema='{"type": "object", "properties": {}}',
        callback=lambda args: {"ok": True},
        metadata=json.dumps(
            {"risk": "low", "side_effects": ["read"], "permissions": ["read"]}
        ),
    )
    assert await agent.add_live_tool(tool) == "lookup"
    assert "lookup" in agent.list_tools()

    definitions = agent.tool_definitions()
    lookup = next(item for item in definitions if item["name"] == "lookup")
    assert lookup["description"] == "Look up a record"
    assert lookup["metadata"]["risk"] == "low"
    assert lookup["metadata"]["permissions"] == ["read"]

    assert agent.remove_live_tools(["lookup"]) == 1
    assert "lookup" not in agent.list_tools()


@pytest.mark.asyncio
async def test_compact_now_on_an_empty_session_returns_none():
    agent = make_agent()
    assert await agent.compact_now(0) is None


def test_plugin_accepts_stream_and_turn_hooks():
    from nbos_native import AgentPlugin

    plugin = AgentPlugin(
        name="observer",
        on_stream_token=lambda token: token,
        on_turn_start=lambda turn: turn,
        on_turn_end=lambda turn: turn,
    )
    assert plugin.get_name() == "observer"


def test_close_releases_runtime_extensions():
    agent = make_agent()
    agent.add_plan_tool()
    assert "update_plan" in agent.list_tools()
    agent.close()
    assert agent.list_tools() == []
