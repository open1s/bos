#!/usr/bin/env python3
"""
Agent Skills Demo — Load skills from directory and use them with LLM

Demonstrates:
1. Creating skill definitions (SKILL.md with YAML frontmatter)
2. Registering skills with the agent via register_skills_from_dir()
3. Running the agent — skills provide context for LLM responses

Skill format:
    skills/
      my-skill/
        SKILL.md          # YAML frontmatter + markdown instructions
        references/       # Optional reference files
          config.toml

Usage:
    export OPENAI_API_KEY="sk-..."
    export LLM_BASE_URL="https://integrate.api.nvidia.com/v1"
    export LLM_MODEL="nvidia/meta/llama-3.1-8b-instruct"
    python3 crates/examples/agent_skill_demo.py
"""

import asyncio
import json
import os
import tempfile
import shutil
from pathlib import Path

from nbos import PyAgent, AgentConfig as PyAgentConfig, Bus as PyBus, BusConfig as PyBusConfig, ConfigLoader, init_tracing

# from nbos import Agent

init_tracing()
loader = ConfigLoader()
loader.discover()
_config = loader.load_sync()
_global = _config.get("global_model", {})

API_KEY = os.environ.get("OPENAI_API_KEY") or _global.get("api_key", "")
BASE_URL = os.environ.get("LLM_BASE_URL") or _global.get("base_url", "https://integrate.api.nvidia.com/v1")
MODEL = os.environ.get("LLM_MODEL") or _global.get("model", "nvidia/meta/llama-3.1-8b-instruct")


def create_sample_skills(skills_dir):

    python_skill = skills_dir / "python-coding"
    python_skill.mkdir(parents=True)
    (python_skill / "SKILL.md").write_text("""\
---
name: python-coding
description: Python coding conventions and best practices for the project
category: coding
version: 1.0.0
author: team
tags: [python, style, conventions]
---

# Python Coding Conventions

## Naming
- Use snake_case for functions and variables
- Use PascalCase for classes
- Use UPPER_CASE for constants

## Error Handling
- Always use specific exception types, never bare `except:`
- Log errors with context before re-raising
- Use context managers for resource cleanup

## Type Hints
- All function signatures must have type hints
- Use `Optional[T]` instead of `T | None` for Python 3.9 compatibility
- Return types are mandatory

## Testing
- One test per function
- Use pytest fixtures for setup
- Mock external services
""")

    db_skill = skills_dir / "database-ops"
    db_skill.mkdir(parents=True)
    (db_skill / "SKILL.md").write_text("""\
---
name: database-ops
description: Database query patterns and connection management
category: infrastructure
version: 1.0.0
author: team
tags: [database, sql, connection]
requires: []
provides: [db-queries, db-connections]
---

# Database Operations

## Connection Pool
- Use connection pooling (max 10 connections)
- Set connection timeout to 30 seconds
- Always return connections to pool after use

## Query Patterns
- Use parameterized queries to prevent SQL injection
- Batch inserts for >100 rows
- Use transactions for multi-table operations

## Error Recovery
- Retry transient errors with exponential backoff
- Max 3 retries before failing
- Log query parameters (excluding secrets) on failure
""")

    api_skill = skills_dir / "api-design"
    api_skill.mkdir(parents=True)
    (api_skill / "SKILL.md").write_text("""\
---
name: api-design
description: REST API design patterns and conventions
category: architecture
version: 1.0.0
author: team
tags: [api, rest, design]
---

# API Design Patterns

## Endpoints
- Use nouns for resources: `/users`, `/orders`
- Use HTTP methods correctly: GET, POST, PUT, DELETE
- Version APIs in URL: `/v1/users`

## Responses
- Always return consistent JSON structure
- Include pagination metadata for list endpoints
- Use proper HTTP status codes

## Authentication
- Use JWT tokens in Authorization header
- Token expiry: 1 hour
- Refresh tokens valid for 7 days
""")


async def demo_skills():
    print("═" * 60)
    print("  Demo — Agent with Skills")
    print("═" * 60)

    skills_dir = Path(tempfile.mkdtemp(prefix="brainos_skills_"))
    print(f"\n  📁 Creating skills in: {skills_dir}")
    create_sample_skills(skills_dir)

    bus = await PyBus.create(PyBusConfig())

    config = PyAgentConfig(
        name="skill-agent",
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY,
        system_prompt=(
            "You are a helpful assistant. "
            "Use the loaded skills as context when answering questions. "
            "Reference specific skill instructions when relevant. "
            "Format: Thought: <reasoning>\nFinal Answer: <response>"
        ),
        temperature=0.7,
        timeout_secs=120,
    )
    agent = await PyAgent.create(config, bus)
    print("  🤖 Agent created")

    await agent.register_skills_from_dir(str(skills_dir))
    print("  📚 Skills registered from directory")

    prompts = [
        ("Python Style", "What are the Python naming conventions for this project?"),
        ("Database", "How should I handle database connections and retries?"),
        ("API Design", "What's the correct way to version our REST API?"),
    ]

    for label, prompt in prompts:
        print(f"\n  ── {label} ──")
        print(f"  📤 User: {prompt}")
        try:
            reply = await agent.react(prompt)
            print(f"  📥 Agent: {reply[:300]}")
        except Exception as e:
            err_str = str(e)
            print(f"  ⚠️  {e}")

    shutil.rmtree(skills_dir, ignore_errors=True)
    print(f"\n  ✅ Skills demo done\n")


async def demo_skills_with_tools():
    print("═" * 60)
    print("  Demo — Tools via react()")
    print("═" * 60)

    bus = await PyBus.create(PyBusConfig())
    config = PyAgentConfig(
        name="tool-agent",
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY,
        system_prompt=(
            "You are a helpful assistant. "
            "Use the calc tool for arithmetic calculations."
        ),
        temperature=0.7,
        timeout_secs=120,
    )
    agent = await PyAgent.create(config, bus)
    print("  🤖 Agent created")

    from nbos import PythonTool

    def calc_callback(args):
        a, b, op = args.get("a", 0), args.get("b", 0), args.get("op", "add")
        ops = {"add": a + b, "sub": a - b, "mul": a * b, "div": a / b if b else "error"}
        return json.dumps({"result": ops.get(op, "unknown")})

    calc_tool = PythonTool(
        name="calc",
        description="Perform arithmetic: add, sub, mul, div",
        parameters=json.dumps({"a": "number", "b": "number", "op": "string"}),
        schema=json.dumps({
            "type": "object",
            "properties": {
                "a": {"type": "number"},
                "b": {"type": "number"},
                "op": {"type": "string", "enum": ["add", "sub", "mul", "div"]}
            },
            "required": ["a", "b", "op"]
        }),
        callback=calc_callback,
    )
    await agent.add_tool(calc_tool)
    print("  🔧 Registered calc tool")

    tools = agent.list_tools()
    print(f"  📋 Tools: {tools}")

    prompts = [
        ("Tool Call", "What is 7 * 8? Use the calc tool with a=7, b=8, op=mul."),
    ]

    for label, prompt in prompts:
        print(f"\n  ── {label} ──")
        print(f"  📤 User: {prompt}")
        try:
            reply = await agent.react(prompt)
            print(f"  📥 Agent: {reply[:400]}")
        except Exception as e:
            print(f"  ⚠️  {e}")

    print(f"\n  ✅ Tools demo done\n")


async def main():
    print("\n" + "📚" * 30)
    print("  BrainOS — Agent Skills Demo")
    print("📚" * 30 + "\n")

    if not API_KEY:
        print("  ⚠️  OPENAI_API_KEY not set — LLM calls will fail")
        print("  Set: export OPENAI_API_KEY=sk-...\n")

    await demo_skills()
    # await demo_skills_with_tools()

    print("═" * 60)
    print("  ✅ All Skills demos completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    asyncio.run(main())
