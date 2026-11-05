"""Full Ferris host: package store, activation and invocation."""

import json
import os
import pathlib
import tempfile

import pytest

from nbos import FerrisHost

MANIFEST = """
schema_version = "2.0"

[extension]
id = "com.example.echo"
name = "Echo"
version = "1.0.0"
description = "Echoes its input"

[compatibility]
host = ">=3.0.0"
extension_api = ">=1.0.0"

[runtime]
kind = "declarative"

[capabilities]
required = ["agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"
"""


PROCESS_MANIFEST = """
schema_version = "2.0"

[extension]
id = "com.example.process"
name = "Process"
version = "1.0.0"
description = "Process echo"

[compatibility]
host = ">=3.0.0"
extension_api = ">=1.0.0"

[runtime]
kind = "process"

[runtime.config]
entry = "server.py"

[capabilities]
required = ["process.execute"]

[[contribution]]
kind = "tool"
id = "echo"
"""

PROCESS_SERVER = """#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": "1.0",
            "token": request.get("params", {}).get("token"),
        }
    elif method == "tools/call":
        result = request.get("params", {}).get("arguments")
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)
"""


def make_process_package() -> str:
    root = tempfile.mkdtemp(prefix="ferris-proc-")
    with open(os.path.join(root, "extension.toml"), "w", encoding="utf-8") as handle:
        handle.write(PROCESS_MANIFEST)
    os.makedirs(os.path.join(root, "tools"))
    with open(os.path.join(root, "tools", "echo.json"), "w", encoding="utf-8") as handle:
        json.dump({"description": "Echo"}, handle)
    server = os.path.join(root, "server.py")
    with open(server, "w", encoding="utf-8") as handle:
        handle.write(PROCESS_SERVER)
    os.chmod(server, 0o755)
    return root


def make_package() -> str:
    root = tempfile.mkdtemp(prefix="ferris-pkg-")
    with open(os.path.join(root, "extension.toml"), "w", encoding="utf-8") as handle:
        handle.write(MANIFEST)
    os.makedirs(os.path.join(root, "tools"))
    with open(os.path.join(root, "tools", "echo.json"), "w", encoding="utf-8") as handle:
        json.dump({"description": "Echo", "implementation": {"kind": "echo_arguments"}}, handle)
    return root


def make_host() -> FerrisHost:
    return FerrisHost(tempfile.mkdtemp(prefix="ferris-host-"))


def test_install_list_plan_and_invoke():
    host = make_host()
    package = make_package()
    installed = host.install(package)
    assert installed == {"id": "com.example.echo", "version": "1.0.0"}

    assert host.installed() == [{"id": "com.example.echo", "version": "1.0.0"}]
    assert host.versions("com.example.echo") == ["1.0.0"]
    assert host.active_version("com.example.echo") == "1.0.0"
    assert os.path.isdir(host.package_root("com.example.echo", "1.0.0"))
    assert host.plan() == ["com.example.echo"]


@pytest.mark.asyncio
async def test_process_runtime_starts_and_invokes():
    host = FerrisHost(
        tempfile.mkdtemp(prefix="ferris-host-"),
        policy_mode="advisory",
        process_sandbox=False,
    )
    host.install(make_process_package())
    contributions = await host.apply()
    assert contributions["com.example.process"][0]["id"] == "echo"
    assert await host.invoke(
        "com.example.process", "tool", "echo", {"ok": True}
    ) == {"ok": True}
    await host.unload()


def example_extension(name: str) -> str:
    return str(
        pathlib.Path(__file__).resolve().parents[3]
        / "examples"
        / "extensions"
        / name
    )


@pytest.mark.asyncio
async def test_extism_runtime_starts_and_invokes():
    host = FerrisHost(
        tempfile.mkdtemp(prefix="ferris-host-"), policy_mode="advisory"
    )
    host.install(example_extension("extism-echo"))
    contributions = await host.apply()
    assert contributions["com.example.extism-echo"][0]["id"] == "echo"
    result = await host.invoke(
        "com.example.extism-echo", "tool", "echo", {"x": 1}
    )
    assert result
    await host.unload()


@pytest.mark.asyncio
async def test_mcp_runtime_starts_and_invokes():
    host = FerrisHost(
        tempfile.mkdtemp(prefix="ferris-host-"),
        policy_mode="advisory",
        process_sandbox=False,
    )
    host.install(example_extension("mcp-echo"))
    contributions = await host.apply()
    assert any(
        entry["id"] == "echo" for entry in contributions["com.example.mcp-echo"]
    )
    assert await host.invoke(
        "com.example.mcp-echo", "tool", "echo", {"x": 1}
    )
    await host.unload()


@pytest.mark.asyncio
async def test_cordis_runtime_starts_and_invokes():
    host = FerrisHost(
        tempfile.mkdtemp(prefix="ferris-host-"),
        policy_mode="advisory",
        process_sandbox=False,
    )
    host.install(example_extension("cordis-echo"))
    contributions = await host.apply()
    assert contributions["com.example.cordis-echo"][0]["id"] == "echo"
    result = await host.invoke(
        "com.example.cordis-echo", "tool", "echo", {"value": "x"}
    )
    assert result["echoed"] == "x"
    await host.unload()


@pytest.mark.asyncio
async def test_wasm_component_runtime_starts_and_invokes():
    host = FerrisHost(
        tempfile.mkdtemp(prefix="ferris-host-"), policy_mode="advisory"
    )
    host.install(example_extension("wasm-echo"))
    contributions = await host.apply()
    assert contributions["com.example.wasm-echo"][0]["id"] == "echo"
    assert await host.invoke(
        "com.example.wasm-echo", "tool", "echo", {"x": 1}
    ) == {"x": 1}
    await host.unload()


@pytest.mark.asyncio
async def test_contribution_callbacks_fire_on_apply_and_unload():
    applied = []
    revoked = []

    def on_contributions(payload):
        applied.append(payload)

    def on_revoke(payload):
        revoked.append(payload)

    host = FerrisHost(
        tempfile.mkdtemp(prefix="ferris-host-"),
        on_contributions=on_contributions,
        on_revoke=on_revoke,
    )
    host.install(make_package())
    await host.apply()
    assert applied[0]["extension"] == "com.example.echo"
    assert applied[0]["contributions"][0]["id"] == "echo"

    await host.unload()
    assert revoked[0]["extension"] == "com.example.echo"


@pytest.mark.asyncio
async def test_event_subscriptions_receive_published_events():
    import asyncio

    events = []
    host = make_host()
    subscription = host.subscribe_events(["demo.*"], events.append)
    report = host.publish_event("demo.ping", {"n": 1})
    assert report["delivered"] == 1

    for _ in range(50):
        if events:
            break
        await asyncio.sleep(0.01)

    assert events[0]["event_type"] == "demo.ping"
    assert events[0]["payload"] == {"n": 1}
    assert host.unsubscribe_events(subscription) is True
    assert host.unsubscribe_events(subscription) is False


@pytest.mark.asyncio
async def test_apply_contributes_and_invokes():
    host = make_host()
    host.install(make_package())

    contributions = await host.apply()
    applied = contributions["com.example.echo"]
    assert len(applied) == 1
    assert applied[0]["kind"] == "tool"
    assert applied[0]["id"] == "echo"
    assert applied[0]["payload"]["description"] == "Echo"

    assert await host.invoke("com.example.echo", "tool", "echo", {"x": 1}) == {"x": 1}
    assert host.contributions()["com.example.echo"][0]["id"] == "echo"

    await host.unload()
    assert host.contributions() == {}
    with pytest.raises(Exception):
        await host.invoke("com.example.echo", "tool", "echo", {})


@pytest.mark.asyncio
async def test_a_rejecting_contribution_callback_leaves_no_phantom_entry():
    def boom(_payload):
        raise RuntimeError("boom")

    host = FerrisHost(
        tempfile.mkdtemp(prefix="ferris-host-"), on_contributions=boom
    )
    host.install(make_package())
    with pytest.raises(Exception):
        await host.apply()
    assert host.contributions() == {}


@pytest.mark.asyncio
async def test_replacing_a_linked_extension_shuts_the_old_one_down():
    revoked = []
    shut_down = []

    def on_revoke(payload):
        revoked.append(payload)

    async def start(_ctx):
        return [{"kind": "tool", "id": "echo", "payload": {}}]

    async def invoke(call):
        return call["input"]

    async def shutdown():
        shut_down.append(True)

    host = FerrisHost(
        tempfile.mkdtemp(prefix="ferris-host-"), on_revoke=on_revoke
    )
    await host.register_extension(
        MANIFEST, tempfile.mkdtemp(prefix="ferris-linked-"), start, invoke, shutdown
    )
    await host.register_extension(
        MANIFEST, tempfile.mkdtemp(prefix="ferris-linked-"), start, invoke, shutdown
    )
    assert len(revoked) == 1
    assert len(shut_down) == 1

    await host.unload()
    assert len(revoked) == 2
    assert len(shut_down) == 2


@pytest.mark.asyncio
async def test_host_registers_applied_tools_on_an_agent_registry():
    class Registry:
        def __init__(self):
            self.tools = []

        def add(self, tool):
            self.tools.append(tool)

    class Agent:
        def __init__(self):
            self._tools = Registry()

    host = make_host()
    host.install(make_package())
    await host.apply()
    agent = Agent()
    registered = await host.register_agent_tools(agent)
    assert registered == [{"extension": "com.example.echo", "name": "echo"}]
    assert len(agent._tools.tools) == 1
    assert agent._tools.tools[0].name == "echo"


@pytest.mark.asyncio
async def test_a_linked_python_extension_registers_and_invokes():
    host = make_host()

    async def start(_ctx):
        return [{"kind": "tool", "id": "echo", "payload": {"description": "Echo"}}]

    async def invoke(call):
        return call["input"]

    contributions = await host.register_extension(
        MANIFEST, tempfile.mkdtemp(prefix="ferris-linked-"), start, invoke
    )
    assert contributions[0]["id"] == "echo"
    assert host.contributions()["com.example.echo"][0]["id"] == "echo"
    assert await host.invoke("com.example.echo", "tool", "echo", {"ok": True}) == {
        "ok": True
    }

    await host.unload()
    assert host.contributions() == {}


@pytest.mark.asyncio
async def test_enable_disable_and_uninstall():
    host = make_host()
    host.install(make_package())
    host.set_enabled("com.example.echo", False)
    assert host.plan() == []
    host.set_enabled("com.example.echo", True)
    assert host.plan() == ["com.example.echo"]
    host.uninstall("com.example.echo", False)
    assert host.installed() == []


def test_an_unknown_grant_scope_is_refused():
    host = make_host()
    host.install(make_package())
    with pytest.raises(Exception):
        host.grant("com.example.echo", "agent.tools.register", "typo-scope")
    assert "agent.tools.register" not in host.grants("com.example.echo")


def test_grants_are_persisted_and_revocable():
    host = make_host()
    host.install(make_package())
    host.grant("com.example.echo", "agent.tools.register", "any", "test")
    grants = host.grants("com.example.echo")
    assert "agent.tools.register" in grants
    assert host.revoke_grant("com.example.echo", "agent.tools.register") is True
    assert host.grants("com.example.echo") == {}
    assert host.audit_degraded() is None
