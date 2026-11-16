"""Agent-runtime controls: plans, goals, steering, deferral, jobs, templates."""

import json

import pytest

from nbos_native import (
    Agent as NativeAgent,
    AgentConfig,
    Goal,
    GoalStore,
    JobRegistry,
    estimate_tokens,
    render_template,
    with_memory,
)


def make_agent() -> NativeAgent:
    config = AgentConfig()
    config.name = "agentic-test"
    config.model = "openai/gpt-4o"
    config.base_url = "http://127.0.0.1:9/v1"
    config.api_key = "test"
    config.system_prompt = "test"
    config.temperature = 0
    config.timeout_secs = 5
    config.api_mode = "chat"
    return NativeAgent.from_config(config)


def test_plan_snapshot_round_trips_and_tool_registers_once():
    agent = make_agent()
    assert agent.plan_items() == []

    agent.set_plan(
        [
            {"text": "write tests", "status": "in_progress"},
            {"text": "ship", "status": "pending"},
        ]
    )
    assert agent.plan_items() == [
        {"text": "write tests", "status": "in_progress"},
        {"text": "ship", "status": "pending"},
    ]

    agent.clear_plan()
    assert agent.plan_items() == []

    assert agent.add_plan_tool() is True
    assert agent.add_plan_tool() is False
    assert "update_plan" in agent.list_tools()


def test_steering_queues_and_returns_undelivered_tickets():
    agent = make_agent()
    assert agent.steer("focus on the parser", 7) is True
    assert agent.steer("then the lexer", 8) is True
    assert agent.steer_depth() == 2
    assert agent.take_undelivered_steers() == [7, 8]
    assert agent.steer_depth() == 0


def test_deferral_withholds_tools_by_name_and_prefix():
    agent = make_agent()
    assert agent.has_deferred_tools() is False
    agent.defer_tool("secret")
    agent.defer_tool_prefix("mcp_")
    assert agent.is_deferred_tool("secret") is True
    assert agent.is_deferred_tool("mcp_search") is True
    assert agent.is_deferred_tool("other") is False
    assert agent.has_deferred_tools() is True


def test_inline_skills_register_list_and_remove():
    agent = make_agent()
    agent.register_skill("reviewer", "Review code", "Read the diff first.")
    skills = agent.list_skills()
    reviewer = next(skill for skill in skills if skill["name"] == "reviewer")
    assert reviewer["description"] == "Review code"
    assert reviewer["instructions"] == "Read the diff first."

    assert agent.remove_skills_by_names(["reviewer"]) == 1
    assert all(skill["name"] != "reviewer" for skill in agent.list_skills())


def test_live_prompt_sections_can_be_added_and_removed():
    agent = make_agent()
    agent.add_live_prompt_section("Stay terse.")
    assert agent.remove_live_prompt_section("Stay terse.") is True
    assert agent.remove_live_prompt_section("Stay terse.") is False


def test_config_carries_new_behaviour_knobs():
    config = AgentConfig()
    config.persona_complete = True
    config.compaction_input_tokens = 9000
    config.tool_output_cap = 4000
    config.parallel_tool_calls = True
    config.web_search = False
    config.max_steps = 4
    config.context_budget = 2048
    config.skill_catalogue = True
    config.add_fallback("http://127.0.0.1:10/v1", "openai/gpt-4o-mini", "key")

    assert config.fallback_list() == [
        {
            "base_url": "http://127.0.0.1:10/v1",
            "api_key": "key",
            "model": "openai/gpt-4o-mini",
        }
    ]

    agent = NativeAgent.from_config(config)
    resolved = agent.config()
    assert resolved["persona_complete"] is True
    assert resolved["compaction_input_tokens"] == 9000
    assert resolved["tool_output_cap"] == 4000
    assert resolved["parallel_tool_calls"] is True
    assert resolved["max_steps"] == 4
    assert resolved["context_budget"] == 2048
    assert resolved["skill_catalogue"] is True
    assert resolved["fallbacks"][0]["model"] == "openai/gpt-4o-mini"


def test_context_budget_setters():
    agent = make_agent()
    assert agent.context_budget() == 0
    agent.set_context_budget(4096)
    assert agent.context_budget() == 4096
    assert agent.skill_catalogue() is False
    agent.set_skill_catalogue(True)
    assert agent.skill_catalogue() is True


def test_goals_transition_and_settle_through_their_store():
    goal = Goal.create("Ship the release")
    assert goal.objective == "Ship the release"
    assert goal.state == "active"
    assert goal.activation == "armed"
    assert goal.advance() is True
    assert goal.rounds == 1
    assert goal.pause() is True
    assert goal.state == "paused"
    assert goal.resume() is True
    assert goal.complete() is True
    assert goal.state == "completed"

    restored = Goal.from_json(json.dumps(goal.to_json()))
    assert restored.objective == "Ship the release"
    assert restored.state == "completed"

    active = Goal.create("Fix the bug")
    store = GoalStore(active)
    assert store.complete() is True
    assert store.has_settle() is True
    assert store.take_settle() == {"type": "complete"}
    assert store.has_settle() is False
    assert store.snapshot().state == "completed"


def test_goal_tools_register_against_a_store_once():
    agent = make_agent()
    store = GoalStore(Goal.create("Ship"))
    assert agent.add_goal_tools(store) == 2
    assert agent.add_goal_tools(store) == 0
    assert "get_goal" in agent.list_tools()
    assert "update_goal" in agent.list_tools()


def test_job_tools_register_against_a_roster_once():
    agent = make_agent()
    jobs = JobRegistry()
    assert agent.add_job_tools(jobs, "default") == 2
    assert agent.add_job_tools(jobs, "default") == 0
    assert "job_output" in agent.list_tools()
    assert "job_kill" in agent.list_tools()


@pytest.mark.asyncio
async def test_the_bash_tool_can_route_background_jobs():
    agent = make_agent()
    jobs = JobRegistry()
    await agent.add_bash_tool("bash", None, jobs, "default")
    assert "bash" in agent.list_tools()


def test_job_roster_is_empty_and_bounded():
    jobs = JobRegistry(10, 1024)
    assert jobs.list("default") == []
    assert jobs.running_count("default") == 0
    assert jobs.settled_count("default") == 0
    assert jobs.clear_settled("default") == 0
    assert jobs.get("missing") is None
    assert jobs.output("missing") is None
    assert jobs.kill("missing") is False
    assert isinstance(jobs.version(), int)


def test_prompt_template_helpers_mirror_the_rust_module():
    assert render_template("Hello {{ name }}!", {"name": "BOS"}) == "Hello BOS!"
    with pytest.raises(Exception):
        render_template("Hello {{name}}!", {})
    assert (
        with_memory("You are {{role}}.\n\n{{memory}}", "Remember: tea.")
        == "You are {{role}}.\n\nRemember: tea."
    )
    assert with_memory("base", "Remember: tea.") == "base\n\nRemember: tea."
    assert estimate_tokens("one two three") > 0
