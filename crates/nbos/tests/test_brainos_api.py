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


class TestBusManager:
    """BusManager lifecycle tests (requires nbos extension)"""

    @pytest.mark.asyncio
    async def test_bus_manager_create(self):
        from nbos.bus import BusManager
        async with BusManager() as bus:
            assert bus is not None

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
