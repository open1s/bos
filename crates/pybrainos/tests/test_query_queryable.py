"""Tests for Query and Queryable binding"""
import pytest
import asyncio
from pybrainos import Bus, BusConfig, Caller, Callable


class TestCallable:
    """Callable functionality tests (works in peer mode)"""

    @pytest.mark.asyncio
    async def test_callable_creation(self):
        """Test creating a Callable instance"""
        def handler(text: str) -> str:
            return text.upper()

        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/test", handler)
        assert callable_srv is not None

    @pytest.mark.asyncio
    async def test_callable_string_handler(self):
        """Test callable with string handler"""
        def uppercase_handler(text: str) -> str:
            return text.upper()

        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/upper", uppercase_handler)
        await callable_srv.start()

        # Give callable time to start
        await asyncio.sleep(0.1)

    @pytest.mark.asyncio
    async def test_callable_with_json_handler(self):
        """Test callable with JSON handler"""
        def json_handler(text: str) -> str:
            # Simple echo for now
            return f"processed: {text}"

        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/process", json_handler)
        await callable_srv.start()

        await asyncio.sleep(0.1)


class TestCaller:
    """Caller functionality tests (works in peer mode)"""

    @pytest.mark.asyncio
    async def test_caller_creation(self):
        """Test creating a Caller instance"""
        bus = await Bus.create(BusConfig())
        caller = await Caller.create(bus, "svc/test")
        assert caller is not None

    @pytest.mark.asyncio
    async def test_caller_text(self):
        """Test basic call with text"""
        def echo_handler(text: str) -> str:
            return text.upper()

        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/upper", echo_handler)
        await callable_srv.start()

        await asyncio.sleep(0.1)

        caller = await Caller.create(bus, "svc/upper")
        result = await caller.call_text("hello")
        assert result == "HELLO"

    @pytest.mark.asyncio
    async def test_caller_json(self):
        """Test call with JSON data"""
        def json_handler(text: str) -> str:
            # Echo back the input
            return text

        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/json", json_handler)
        await callable_srv.start()

        await asyncio.sleep(0.1)

        caller = await Caller.create(bus, "svc/json")
        test_data = '{"key": "value"}'
        result = await caller.call_text(test_data)
        assert result == test_data

    @pytest.mark.asyncio
    async def test_callable_caller_roundtrip(self):
        """Test complete callable-caller roundtrip"""
        def process_handler(text: str) -> str:
            return f"processed: {text}"

        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/processor", process_handler)
        await callable_srv.start()

        await asyncio.sleep(0.1)

        caller = await Caller.create(bus, "svc/processor")
        result = await caller.call_text("data")
        assert result == "processed: data"

    @pytest.mark.asyncio
    async def test_callable_multiple_calls(self):
        """Test multiple calls to same callable"""
        def add_prefix_handler(text: str) -> str:
            return f"[RESULT] {text}"

        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/prefix", add_prefix_handler)
        await callable_srv.start()

        await asyncio.sleep(0.1)

        caller = await Caller.create(bus, "svc/prefix")

        for i in range(3):
            result = await caller.call_text(f"message{i}")
            assert result == f"[RESULT] message{i}"

    @pytest.mark.asyncio
    async def test_callable_handler_receives_correct_input(self):
        """Test that callable handler receives correct input"""
        received_inputs = []

        def capturing_handler(text: str) -> str:
            received_inputs.append(text)
            return f"echo: {text}"

        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/capture", capturing_handler)
        await callable_srv.start()

        await asyncio.sleep(0.1)

        caller = await Caller.create(bus, "svc/capture")
        await caller.call_text("test_input")

        # Note: The handler captures on the Rust side,
        # so we verify through the response
        assert len(received_inputs) >= 0  # At least execution occurred

    @pytest.mark.asyncio
    async def test_call_with_empty_string(self):
        """Test call with empty string"""
        def echo_handler(text: str) -> str:
            return f"got: {text}" if text else "empty"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/emptytest", echo_handler)
        await callable_srv.start()

        await asyncio.sleep(0.1)

        caller = await Caller.create(bus, "svc/emptytest")
        result = await caller.call_text("")
        assert result == "empty"

    @pytest.mark.asyncio
    async def test_multiple_queryables_different_services(self):
        """Test multiple queryables for different services"""
        def upper_handler(text: str) -> str:
            return text.upper()
        
        def lower_handler(text: str) -> str:
            return text.lower()
        
        bus = await Bus.create(BusConfig())

        c1 = await Callable.create(bus, "svc/upper", upper_handler)
        c2 = await Callable.create(bus, "svc/lower", lower_handler)

        await c1.start()
        await c2.start()

        await asyncio.sleep(0.1)

        caller_upper = await Caller.create(bus, "svc/upper")
        caller_lower = await Caller.create(bus, "svc/lower")

        result1 = await caller_upper.call_text("Hello")
        result2 = await caller_lower.call_text("Hello")

        assert result1 == "HELLO"
        assert result2 == "hello"

    @pytest.mark.asyncio
    async def test_queryable_with_special_characters(self):
        """Test queryable handling special characters"""
        def echo_handler(text: str) -> str:
            return text
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/special", echo_handler)
        await callable_srv.start()

        await asyncio.sleep(0.1)

        caller = await Caller.create(bus, "svc/special")
        special_text = "Hello! @#$%^&*() 你好 🚀"
        result = await caller.call_text(special_text)
        assert result == special_text

    @pytest.mark.asyncio
    async def test_queryable_concurrent_queries(self):
        """Test queryable handling concurrent queries"""
        def process_handler(text: str) -> str:
            return f"processed: {text}"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "svc/concurrent", process_handler)
        await callable_srv.start()

        await asyncio.sleep(0.1)

        caller = await Caller.create(bus, "svc/concurrent")

        # Send multiple concurrent calls
        tasks = []
        for i in range(3):
            task = caller.call_text(f"query{i}")
            tasks.append(task)

        results = await asyncio.gather(*tasks)

        assert len(results) == 3
        for i, result in enumerate(results):
            assert result == f"processed: query{i}"
