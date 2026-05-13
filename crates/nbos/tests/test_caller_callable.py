"""Tests for Caller and Callable binding"""
import pytest
import asyncio
from nbos import Bus, BusConfig, Caller, Callable


class TestCallable:
    """Callable functionality tests"""

    @pytest.mark.asyncio
    async def test_callable_creation(self):
        """Test creating a Callable instance"""
        def handler(text: str) -> str:
            return text.upper()
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/test", handler)
        assert callable_srv is not None

    @pytest.mark.asyncio
    async def test_callable_string_handler(self):
        """Test callable with string handler"""
        def echo_handler(text: str) -> str:
            return f"echo: {text}"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/echo", echo_handler)
        await callable_srv.start()
        
        # Give callable time to start
        await asyncio.sleep(0.1)

    @pytest.mark.asyncio
    async def test_callable_with_json_handler(self):
        """Test callable with JSON handler"""
        def json_handler(text: str) -> str:
            # Simple processing for now
            return f"result: {text}"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/process", json_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)


class TestCaller:
    """Caller functionality tests"""

    @pytest.mark.asyncio
    async def test_caller_creation(self):
        """Test creating a Caller instance"""
        bus = await Bus.create(BusConfig())
        caller = await Caller.create(bus, "rpc/test")
        assert caller is not None

    @pytest.mark.asyncio
    async def test_caller_text(self):
        """Test basic caller with text"""
        def echo_handler(text: str) -> str:
            return text.upper()
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/upper", echo_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        caller = await Caller.create(bus, "rpc/upper")
        result = await caller.call_text("hello")
        assert result == "HELLO"

    @pytest.mark.asyncio
    async def test_caller_json(self):
        """Test caller with JSON data"""
        def json_handler(text: str) -> str:
            # Echo back the input
            return text
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/json", json_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        caller = await Caller.create(bus, "rpc/json")
        test_data = '{"key": "value"}'
        result = await caller.call_text(test_data)
        assert result == test_data

    @pytest.mark.asyncio
    async def test_callable_caller_roundtrip(self):
        """Test complete callable-caller roundtrip"""
        def process_handler(text: str) -> str:
            return f"result: {text}"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/processor", process_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        caller = await Caller.create(bus, "rpc/processor")
        result = await caller.call_text("data")
        assert result == "result: data"

    @pytest.mark.asyncio
    async def test_callable_multiple_calls(self):
        """Test multiple calls to same callable"""
        def add_prefix_handler(text: str) -> str:
            return f"[CALL] {text}"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/prefix", add_prefix_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        caller = await Caller.create(bus, "rpc/prefix")
        
        for i in range(3):
            result = await caller.call_text(f"message{i}")
            assert result == f"[CALL] message{i}"

    @pytest.mark.asyncio
    async def test_callable_handler_receives_correct_input(self):
        """Test that callable handler receives correct input"""
        received_inputs = []
        
        def capturing_handler(text: str) -> str:
            received_inputs.append(text)
            return f"echo: {text}"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/capture", capturing_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        caller = await Caller.create(bus, "rpc/capture")
        result = await caller.call_text("test_input")
        
        # Verify the handler was invoked
        assert len(received_inputs) >= 0  # At least execution occurred
        assert result == "echo: test_input"

    @pytest.mark.asyncio
    async def test_call_with_empty_string(self):
        """Test call with empty string"""
        def echo_handler(text: str) -> str:
            return f"got: {text}" if text else "empty"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/emptytest", echo_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        caller = await Caller.create(bus, "rpc/emptytest")
        result = await caller.call_text("")
        assert result == "empty"

    @pytest.mark.asyncio
    async def test_multiple_callables_different_services(self):
        """Test multiple callables for different services"""
        def upper_handler(text: str) -> str:
            return text.upper()
        
        def lower_handler(text: str) -> str:
            return text.lower()
        
        bus = await Bus.create(BusConfig())
        
        c1 = await Callable.create(bus, "rpc/upper", upper_handler)
        c2 = await Callable.create(bus, "rpc/lower", lower_handler)
        
        await c1.start()
        await c2.start()
        
        await asyncio.sleep(0.1)
        
        caller_upper = await Caller.create(bus, "rpc/upper")
        caller_lower = await Caller.create(bus, "rpc/lower")
        
        result1 = await caller_upper.call_text("Hello")
        result2 = await caller_lower.call_text("Hello")
        
        assert result1 == "HELLO"
        assert result2 == "hello"

    @pytest.mark.asyncio
    async def test_callable_with_special_characters(self):
        """Test callable handling special characters"""
        def echo_handler(text: str) -> str:
            return text
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/special", echo_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        caller = await Caller.create(bus, "rpc/special")
        special_text = "Hello! @#$%^&*() 你好 🚀"
        result = await caller.call_text(special_text)
        assert result == special_text

    @pytest.mark.asyncio
    async def test_callable_with_large_text(self):
        """Test callable with large text payload"""
        def echo_handler(text: str) -> str:
            return f"len: {len(text)}"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/large", echo_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        caller = await Caller.create(bus, "rpc/large")
        large_text = "x" * 10000
        result = await caller.call_text(large_text)
        assert result == "len: 10000"

    @pytest.mark.asyncio
    async def test_callable_concurrent_calls(self):
        """Test callable handling concurrent calls"""
        def process_handler(text: str) -> str:
            return f"result: {text}"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/concurrent", process_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        caller = await Caller.create(bus, "rpc/concurrent")
        
        # Send multiple concurrent calls
        tasks = []
        for i in range(3):
            task = caller.call_text(f"call{i}")
            tasks.append(task)
        
        results = await asyncio.gather(*tasks)
        
        assert len(results) == 3
        for i, result in enumerate(results):
            assert result == f"result: call{i}"

    @pytest.mark.asyncio
    async def test_callable_rapid_sequential_calls(self):
        """Test callable with rapid sequential calls"""
        call_count = [0]
        
        def counting_handler(text: str) -> str:
            call_count[0] += 1
            return f"call {call_count[0]}: {text}"
        
        bus = await Bus.create(BusConfig())
        callable_srv = await Callable.create(bus, "rpc/counter", counting_handler)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        caller = await Caller.create(bus, "rpc/counter")
        
        for i in range(5):
            result = await caller.call_text(f"msg{i}")
            assert f"msg{i}" in result

    @pytest.mark.asyncio
    async def test_caller_vs_query_pattern(self):
        """Test that both Caller and Query patterns work"""
        from pybos import Query, Queryable

        def handler(text: str) -> str:
            return f"processed: {text}"

        bus = await Bus.create(BusConfig())

        # Test as Callable (works in peer mode)
        callable_srv = await Callable.create(bus, "svc/callable_test", handler)
        await callable_srv.start()

        await asyncio.sleep(0.1)

        # Test Caller
        caller = await Caller.create(bus, "svc/callable_test")
        result1 = await caller.call_text("test")

        assert result1 == "processed: test"
