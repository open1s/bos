"""Ferris SDK authoring binding: Python extensions run under FerrisHarness."""

import pytest

from nbos import FerrisHarness, FerrisManifest, FerrisManifestBuilder

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
kind = "builtin"

[capabilities]
required = ["agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"

[contribution.payload]
description = "Echo"
"""


async def start(ctx):
    assert ctx["id"] == "com.example.echo"
    assert ctx["root"]
    return [{"kind": "tool", "id": "echo", "payload": {"description": "Echo"}}]


async def invoke(call):
    return call["input"]


def test_manifest_parses_and_exposes_identity():
    manifest = FerrisManifest.parse(MANIFEST)
    assert manifest.id == "com.example.echo"
    assert manifest.runtime_kind == "builtin"
    assert "[[contribution]]" in manifest.to_toml()
    value = manifest.to_json()
    assert value["extension"]["id"] == "com.example.echo"
    assert value["runtime"]["kind"] == "builtin"


def test_the_chainable_builder_produces_a_valid_manifest():
    manifest = (
        FerrisManifestBuilder(
            "com.example.built", "Built", "1.2.3", "A built manifest"
        )
        .host(">=3.0.0")
        .runtime("declarative")
        .capability("agent.tools.register")
        .contribution("tool", "echo", {"description": "Echo"})
        .author("Ferris")
        .license("MIT")
        .max_memory_mb(64)
        .max_execution_ms(5000)
        .startup("on-demand")
        .build()
    )

    assert manifest.id == "com.example.built"
    assert manifest.runtime_kind == "declarative"
    toml = manifest.to_toml()
    assert "com.example.built" in toml
    assert "agent.tools.register" in toml
    assert "max_memory_mb = 64" in toml


def test_invalid_manifest_is_refused_at_construction():
    with pytest.raises(Exception):
        FerrisManifest.parse('schema_version = "1.0"')
    with pytest.raises(Exception):
        FerrisHarness("not toml", "/tmp/ferris-py", start, invoke)


@pytest.mark.asyncio
async def test_python_extension_starts_invokes_and_shuts_down():
    stopped = []

    async def shutdown():
        stopped.append(True)

    harness = FerrisHarness(
        MANIFEST, "/tmp/ferris-py-test", start, invoke, shutdown
    )
    assert harness.runtime_kind == "builtin"
    assert harness.root == "/tmp/ferris-py-test"
    assert harness.manifest().id == "com.example.echo"

    contributions = await harness.start()
    assert contributions == [
        {"kind": "tool", "id": "echo", "payload": {"description": "Echo"}}
    ]

    assert await harness.invoke("echo", {"text": "hi"}) == {"text": "hi"}
    assert await harness.invoke_contribution("tool", "echo", {"nested": [1, 2]}) == {
        "nested": [1, 2]
    }

    await harness.shutdown()
    assert stopped == [True]


@pytest.mark.asyncio
async def test_sync_callbacks_work_too():
    harness = FerrisHarness(
        MANIFEST,
        "/tmp/ferris-py-sync",
        lambda ctx: [{"kind": "tool", "id": "echo", "payload": {}}],
        lambda call: call["input"],
    )
    contributions = await harness.start()
    assert len(contributions) == 1
    assert await harness.invoke("echo", {"ok": True}) == {"ok": True}
    await harness.shutdown()


@pytest.mark.asyncio
async def test_a_callback_error_surfaces_as_an_exception():
    async def failing(_call):
        raise RuntimeError("invoke failed")

    harness = FerrisHarness(
        MANIFEST, "/tmp/ferris-py-error", start, failing
    )
    await harness.start()
    with pytest.raises(Exception):
        await harness.invoke("echo", {})
