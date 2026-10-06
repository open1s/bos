"""Tests for the brainos high-level Python API (wrappers, decorators, builders)."""
import pytest
import asyncio
import json
from nbos import BrainOS, tool, ToolDef, ToolResult, ToolRegistry
from nbos.tool import _extract_params, _build_schema
from nbos.config import Config


class TestToolDecorator:
    """@tool decorator functionality"""

    def test_simple_tool(self):
        """Test basic @tool decorator"""
        @tool("Add two numbers")
        def add(a: int, b: int) -> int:
            return a + b

        assert isinstance(add, ToolDef)
        assert add.name == "add"
        assert add.description == "Add two numbers"

    def test_tool_with_custom_name(self):
        """Test @tool with explicit name override"""
        @tool("Multiply", name="multiply")
        def mul(a: int, b: int) -> int:
            return a * b

        assert mul.name == "multiply"

    def test_tool_callback_returns_json(self):
        """Test tool callback serializes to JSON"""
        @tool("Get info")
        def get_info(name: str) -> dict:
            return {"name": name, "status": "ok"}

        result = get_info.callback({"name": "test"})
        parsed = json.loads(result)
        assert parsed["name"] == "test"
        assert parsed["status"] == "ok"

    def test_tool_callback_returns_string(self):
        """Test tool callback returns string directly"""
        @tool("Echo")
        def echo(message: str) -> str:
            return message

        result = echo.callback({"message": "hello"})
        assert result == "hello"

    def test_tool_callback_with_type_coercion(self):
        """Test type coercion in tool callback"""
        @tool("Calculate")
        def calc(a: int, b: float) -> float:
            return a + b

        result = calc.callback({"a": "5", "b": "3.5"})
        parsed = json.loads(result)
        assert parsed == 8.5

    def test_tool_parameters_extracted(self):
        """Test parameter extraction from function signature"""
        @tool("Test params")
        def example(name: str, count: int = 0, flag: bool = False) -> dict:
            return {"name": name, "count": count}

        assert "name" in example.parameters
        assert example.parameters["name"]["type"] == "string"
        assert "count" in example.parameters
        assert example.parameters["count"]["type"] == "integer"
        assert "flag" in example.parameters
        assert example.parameters["flag"]["type"] == "boolean"
        assert example.schema["required"] == ["name"]

    def test_tool_result_success(self):
        """Test ToolResult.success factory"""
        r = ToolResult.success({"key": "value"})
        assert r.success is True
        assert r.data == {"key": "value"}
        assert r.error is None

    def test_tool_result_error(self):
        """Test ToolResult.error factory"""
        r = ToolResult.error("something went wrong")
        assert r.success is False
        assert r.data is None
        assert r.error == "something went wrong"


class TestToolRegistry:
    """ToolRegistry functionality"""

    def test_registry_empty(self):
        r = ToolRegistry()
        assert r.size() == 0
        assert r.list() == []

    def test_registry_add_and_get(self):
        r = ToolRegistry()
        t = ToolDef(name="test", description="A test tool", callback=lambda x: x)
        r.add(t)
        assert r.size() == 1
        assert r.has("test")
        assert r.get("test") is t

    def test_registry_remove(self):
        r = ToolRegistry()
        t = ToolDef(name="test", description="A test tool", callback=lambda x: x)
        r.add(t)
        r.remove("test")
        assert not r.has("test")

    def test_registry_merge(self):
        r1 = ToolRegistry()
        r1.add(ToolDef(name="a", description="Tool A", callback=lambda x: x))
        r2 = ToolRegistry()
        r2.add(ToolDef(name="b", description="Tool B", callback=lambda x: x))
        r1.merge(r2)
        assert r1.size() == 2
        assert r1.has("a")
        assert r1.has("b")

    def test_registry_clear(self):
        r = ToolRegistry()
        r.add(ToolDef(name="a", description="Tool A", callback=lambda x: x))
        r.add(ToolDef(name="b", description="Tool B", callback=lambda x: x))
        r.clear()
        assert r.size() == 0

    def test_registry_list_tools(self):
        r = ToolRegistry()
        t = ToolDef(name="test", description="A test tool", callback=lambda x: x)
        r.add(t)
        tools = r.list_tools()
        assert len(tools) == 1
        assert tools[0] is t

    def test_registry_from_list(self):
        tools = [
            ToolDef(name="a", description="Tool A", callback=lambda x: x),
            ToolDef(name="b", description="Tool B", callback=lambda x: x),
        ]
        r = ToolRegistry(tools)
        assert r.size() == 2

    def test_registry_unregister_and_list_tool_defs(self):
        r = ToolRegistry()
        t = ToolDef(name="test", description="A test tool", callback=lambda x: x)
        r.add(t)
        assert r.list_tool_defs() == [t]
        assert r.unregister("test") is r
        assert not r.has("test")

    def test_registry_filter(self):
        r = ToolRegistry(
            [
                ToolDef(name="a", description="Tool A", callback=lambda x: x),
                ToolDef(name="b", description="Tool B", callback=lambda x: x),
            ]
        )
        only_a = r.filter(lambda t: t.name == "a")
        assert only_a.list() == ["a"]
        # filter returns a new registry; the original is untouched
        assert r.size() == 2

    def test_registry_list_by_category(self):
        r = ToolRegistry()
        r.add(ToolDef(name="plain", description="No category", callback=lambda x: x))
        tagged = ToolDef(name="cat", description="Has category", callback=lambda x: x)
        tagged.category = "search"
        r.add(tagged)
        assert [t.name for t in r.list_by_category("search")] == ["cat"]
        assert r.list_by_category("missing") == []

    def test_registry_to_json(self):
        r = ToolRegistry([ToolDef(name="a", description="Tool A", callback=lambda x: x)])
        assert r.to_json() == [
            {"name": "a", "description": "Tool A", "parameters": {}, "schema": {}}
        ]


class TestConfig:
    """Config wrapper functionality"""

    def test_config_create(self):
        cfg = Config()
        assert cfg is not None

    def test_config_inline(self):
        cfg = Config()
        cfg.add_inline({"key": "value", "number": 42})
        data = cfg.load_sync()
        assert data["key"] == "value"
        assert data["number"] == 42

    def test_config_reset(self):
        cfg = Config()
        cfg.add_inline({"version": 1})
        cfg.reset()
        cfg.add_inline({"version": 2})
        data = cfg.load_sync()
        assert data["version"] == 2

    def test_config_reload(self):
        cfg = Config()
        cfg.add_inline({"key": "initial"})
        data1 = cfg.load_sync()
        assert data1["key"] == "initial"
        data2 = cfg.reload_sync()
        assert data2["key"] == "initial"

    def test_config_fluent_load_and_accessors(self):
        cfg = Config.from_inline(
            {
                "global_model": {"model": "m", "base_url": "u", "api_key": "k"},
                "bus": {"x": 1},
            }
        )
        assert cfg.is_loaded() is True
        assert cfg.model == "m"
        assert cfg.base_url == "u"
        assert cfg.api_key == "k"
        assert cfg.bus == {"x": 1}
        assert cfg.global_model["model"] == "m"
        assert cfg.to_json()["bus"] == {"x": 1}

    def test_config_get_defaults_and_lazy_load(self):
        from nbos.config import DEFAULT_MODEL

        cfg = Config()
        cfg.add_inline({"a": {"b": 7}})
        # get() loads lazily on first use
        assert cfg.get("a.b") == 7
        assert cfg.get("a.missing", "fallback") == "fallback"
        assert cfg.is_loaded() is True
        # falls back to the documented default when global_model is absent
        assert cfg.model == DEFAULT_MODEL

    def test_config_aliases_match_fluent_names(self):
        cfg = Config()
        assert cfg.add_inline({"v": 1}) is cfg
        assert cfg.inline({"w": 2}) is cfg
        assert cfg.load() is cfg
        assert cfg.get("v") == 1
        assert cfg.get("w") == 2


class TestBrainOSFacade:
    """BrainOS lifecycle and entry-point parity"""

    @pytest.mark.asyncio
    async def test_create_starts_and_stops(self):
        brain = await BrainOS.create()
        assert brain.is_started is True
        assert isinstance(brain.config, Config)
        assert brain.registry is not None
        await brain.stop()
        assert brain.is_started is False

    @pytest.mark.asyncio
    async def test_start_is_idempotent(self):
        brain = BrainOS()
        assert brain.is_started is False
        await brain.start()
        await brain.start()
        assert brain.is_started is True
        await brain.stop()
        assert brain.is_started is False

    @pytest.mark.asyncio
    async def test_create_bus_returns_started_manager(self):
        manager = await BrainOS().create_bus()
        assert manager.bus is not None
        await manager.stop()


class TestBusManager:
    """BusManager lifecycle tests (requires nbos extension)"""

    @pytest.mark.asyncio
    async def test_bus_manager_create(self):
        from nbos.bus import BusManager
        async with BusManager() as bus:
            assert bus is not None

    @pytest.mark.asyncio
    async def test_bus_manager_session_id(self):
        from nbos.bus import BusManager
        async with BusManager() as bus:
            assert isinstance(bus.session_id, str)
            assert bus.session_id
            assert bus.session_id == bus.bus.session_id()

    @pytest.mark.asyncio
    async def test_bus_manager_publish_text(self):
        from nbos.bus import BusManager
        async with BusManager() as bus:
            sub = await bus.create_subscriber("test/topic")
            await bus.publish_text("test/topic", "hello")
            msg = await sub.recv_with_timeout_ms(1000)
            assert msg == "hello"

    @pytest.mark.asyncio
    async def test_bus_manager_publish_json(self):
        from nbos.bus import BusManager
        async with BusManager() as bus:
            sub = await bus.create_subscriber("test/json")
            await bus.publish_json("test/json", {"key": "value"})
            msg = await sub.recv_with_timeout_ms(1000)
            assert msg is not None

    @pytest.mark.asyncio
    async def test_publisher_subscriber(self):
        from nbos.bus import BusManager
        async with BusManager() as bus:
            pub = await bus.create_publisher("test/ps")
            sub = await bus.create_subscriber("test/ps")
            await pub.publish_text("payload")
            msg = await sub.recv_with_timeout_ms(1000)
            assert msg == "payload"

    @pytest.mark.asyncio
    async def test_subscriber_recv_json(self):
        from nbos.bus import BusManager
        async with BusManager() as bus:
            pub = await bus.create_publisher("test/json2")
            sub = await bus.create_subscriber("test/json2")
            await pub.publish_json({"action": "test", "id": 42})
            data = await sub.recv_json_with_timeout_ms(1000)
            assert data is not None
            assert data.get("action") == "test"
            assert data.get("id") == 42


class TestQueryCallableHighLevel:
    """High-level Query/Caller via BusManager"""

    @pytest.mark.asyncio
    async def test_queryable_via_bus_manager(self):
        from nbos.bus import BusManager
        async with BusManager() as bus:
            def upper(text: str) -> str:
                return text.upper()

            q = await bus.create_queryable("svc/upper", upper)
            await q.start()

            query = await bus.create_query("svc/upper")
            result = await query.query_text("hello")
            assert result == "HELLO"

    @pytest.mark.asyncio
    async def test_callable_via_bus_manager(self):
        from nbos.bus import BusManager
        async with BusManager() as bus:
            def echo(text: str) -> str:
                return f"echo:{text}"

            srv = await bus.create_callable("rpc/echo", echo)
            await srv.start()

            caller = await bus.create_caller("rpc/echo")
            result = await caller.call_text("ping")
            assert result == "echo:ping"


class TestSessionManager:
    """SessionManager functionality"""

    @pytest.mark.asyncio
    async def test_session_manager_save_and_restore(self, tmp_path):
        from nbos import BrainOS
        from nbos import tool as _tool

        @_tool("Add numbers")
        def add(a: int, b: int) -> int:
            return a + b

        session_file = str(tmp_path / "session.json")

        async with BrainOS() as brain:
            agent = await (
                brain.agent("session-test")
                .with_tools(add)
                .start()
            )
            agent.session.save(session_file)
            agent.session.restore(session_file)

    @pytest.mark.asyncio
    async def test_session_compact_forwards_limits(self):
        from nbos import BrainOS

        async with BrainOS() as brain:
            agent = await brain.agent("compact-test").start()
            for i in range(30):
                agent.session.add_message("user", f"message {i}")
            # The builder defaults used to be dropped and the native layer
            # hard-coded (12, 4000); passing limits must not raise.
            agent.session.compact(keep_recent=5, max_summary_chars=100)
            assert isinstance(agent.session.get_messages(), list)

    @pytest.mark.asyncio
    async def test_session_restore_full(self, tmp_path):
        from nbos import BrainOS

        session_file = str(tmp_path / "session-full.json")

        async with BrainOS() as brain:
            agent = await brain.agent("full-session-test").start()
            agent.session.add_message("user", "hello")
            agent.session.save_full(session_file)
            agent.session.restore_full(session_file)

    @pytest.mark.asyncio
    async def test_session_get_messages(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("test-agent").start()
            msgs = agent.session.get_messages()
            assert isinstance(msgs, list)

    @pytest.mark.asyncio
    async def test_session_add_message(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("test-agent").start()
            agent.session.add_message("user", "Hello")
            msgs = agent.session.get_messages()
            assert any(
                m.get("role") == "user" and m.get("content") == "Hello"
                for m in msgs
            )

    @pytest.mark.asyncio
    async def test_session_context_round_trip(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("test-agent").start()
            assert agent.session.context is None
            agent.session.set_context({"topic": "math"})
            assert agent.session.context == {"topic": "math"}
            agent.session.clear_context()
            assert agent.session.context is None

    @pytest.mark.asyncio
    async def test_session_clear_drops_messages_and_context(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("test-agent").start()
            agent.session.add_message("user", "drop me")
            agent.session.set_context({"topic": "trig"})

            agent.session.clear()
            assert agent.session.get_messages() == []
            assert agent.session.context is None

    @pytest.mark.asyncio
    async def test_session_export_import(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("test-agent").start()
            agent.session.add_message("user", "snapshot me")
            snapshot = agent.session.export()
            assert isinstance(snapshot, dict)
            assert isinstance(agent.session.export_json(), str)
            agent.session.add_message("user", "another")
            agent.session.import_session(snapshot)
            assert len(agent.session.get_messages()) == len(snapshot["messages"])
            agent.session.import_session(agent.session.export_json())
            assert len(agent.session.get_messages()) == len(snapshot["messages"])


class TestAgentAccessors:
    """Read-only agent accessors shared with the JS binding"""

    @pytest.mark.asyncio
    async def test_agent_config_is_dict(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("accessor-test").start()
            assert isinstance(agent.config, dict)

    @pytest.mark.asyncio
    async def test_agent_tool_names(self):
        from nbos import BrainOS
        from nbos import tool as _tool

        @_tool("Add numbers")
        def add(a: int, b: int) -> int:
            return a + b

        async with BrainOS() as brain:
            agent = await brain.agent("accessor-test").with_tools(add).start()
            assert agent.tool_names == ["add"]

    @pytest.mark.asyncio
    async def test_agent_add_remote_agent_tool(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("accessor-test").start()
            assert "remoteEcho" not in agent.tool_names
            await agent._inner.add_remote_agent_tool(
                "remoteEcho", "zenoh/remote_echo", brain._bus
            )
            assert "remoteEcho" in agent.tool_names

    @pytest.mark.asyncio
    async def test_agent_metrics(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("metrics-test").start()
            metrics = agent.metrics
            assert metrics["llm_call_count"] == 0
            assert "total_wall_time_us" in metrics
            assert agent.reset_metrics() is agent

    @pytest.mark.asyncio
    async def test_agent_mcp_listing_empty(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("mcp-test").start()
            assert await agent.list_mcp_tools() == []
            assert await agent.list_mcp_resources("none") == []
            assert await agent.list_mcp_prompts() == []

    @pytest.mark.asyncio
    async def test_agent_builder_with_config_and_resilience(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            builder = brain.agent("cfg").with_config(
                {
                    "temperature": 0.2,
                    "max_tokens": 512,
                    "rate_limit": {"capacity": 10, "window_secs": 5, "max_retries": 2},
                    "circuit_breaker": {"max_failures": 3, "cooldown_secs": 45},
                }
            )
            assert builder._config.temperature == pytest.approx(0.2)
            assert builder._config.max_tokens == 512
            assert builder._config.rate_limit_capacity == 10
            assert builder._config.rate_limit_window_secs == 5
            assert builder._config.circuit_breaker_max_failures == 3
            assert builder._config.circuit_breaker_cooldown_secs == 45
            builder.with_rate_limit(7).with_circuit_breaker(9)
            assert builder._config.rate_limit_capacity == 7
            assert builder._config.circuit_breaker_max_failures == 9
            builder.with_system("be terse")
            assert builder._config.system_prompt == "be terse"

    @pytest.mark.asyncio
    async def test_agent_stop_suppresses_next_call(self):
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("stop-test").start()
            assert agent.is_running is False
            assert agent.stop() is False
            assert await agent.run_simple("hi") == ""


class TestParamExtraction:
    """Parameter extraction utilities"""

    def test_extract_simple_params(self):
        def func(name: str, count: int):
            pass
        result = _extract_params(func)
        assert result["type"] == "object"
        assert "name" in result["properties"]
        assert "count" in result["properties"]
        assert result["required"] == ["name", "count"]

    def test_extract_with_defaults(self):
        def func(name: str, count: int = 0):
            pass
        result = _extract_params(func)
        assert "name" in result["required"]
        assert "count" not in result["required"]
        assert result["properties"]["count"]["default"] == 0

    def test_build_schema(self):
        params = {"type": "object", "properties": {"x": {"type": "string"}}}
        schema = _build_schema(params)
        assert schema is params

    def test_build_schema_from_properties(self):
        props = {"x": {"type": "string"}}
        schema = _build_schema(props)
        assert schema["type"] == "object"
        assert schema["properties"] == props
