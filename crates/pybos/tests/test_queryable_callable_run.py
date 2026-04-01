"""
Tests for Queryable.run() and Callable.run() callback patterns
"""
import asyncio
import json
from pyee import Handler
import pytest
from pybos import Bus, BusConfig


@pytest.mark.asyncio
async def test_callable_run_text_handler():
    """Test Callable.run() with text handler"""
    from pybos import Callable, Caller
    
    bus = await Bus.create(BusConfig())
    
    # Handler that echoes back the request
    def handler(request):
        data = json.loads(request)
        data['echo'] = True
        return json.dumps(data)
    
    # Create callable with run handler
    callable = await Callable.create(bus, "test/echo")
    
    # Start handler in background - wrap in async function for create_task
    async def run_handler():
        try:
            await callable.run(handler)
        except asyncio.CancelledError:
            pass
    
    handler_task = asyncio.create_task(run_handler())
    
    # Give handler time to start and register with Zenoh
    await asyncio.sleep(1.0)
    
    # Make a call from another client
    caller = await Caller.create(bus, "test/echo")
    response = await caller.call_text(json.dumps({"msg": "hello"}))
    
    # Verify response
    result = json.loads(response)
    assert result["msg"] == "hello"
    assert result["echo"] == True
    
    # Cancel handler task
    handler_task.cancel()
    try:
        await handler_task
    except asyncio.CancelledError:
        pass
    
    print("✅ Callable.run() text handler test passed")


@pytest.mark.asyncio
async def test_callable_run_json_handler():
    """Test Callable.run_json() with JSON dict handler"""
    from pybos import Callable, Caller

    bus = await Bus.create(BusConfig())

    # Handler that receives dict, returns dict
    received_dicts = []

    def handler(data):
        """Receives parsed JSON dict"""
        received_dicts.append(data)
        # Return dict (will be JSON serialized)
        return {"processed": True, "id": data.get("id", 0)}

    callable_srv = await Callable.create(bus, "test/json_handler")

    async def run_handler():
        try:
            await callable_srv.run_json(handler)
        except asyncio.CancelledError:
            pass

    handler_task = asyncio.create_task(run_handler())
    await asyncio.sleep(1.0)
    caller = await Caller.create(bus, "test/json_handler")
    resp1 = await caller.call_text(json.dumps({"id": 1, "action": "start"}))
    resp2 = await caller.call_text(json.dumps({"id": 2, "action": "stop"}))
    # Verify responses
    assert json.loads(resp1) == {"processed": True, "id": 1}
    assert json.loads(resp2) == {"processed": True, "id": 2}
    
    # Verify handler received dicts
    assert len(received_dicts) == 2
    assert received_dicts[0]["id"] == 1
    assert received_dicts[1]["id"] == 2
    
    handler_task.cancel()
    try:
        await handler_task
    except asyncio.CancelledError:
        pass
    
    print("✅ Queryable.run_json() handler test passed")


@pytest.mark.asyncio
async def test_callable_run_text_handler():
    """Test Callable.run() with text handler"""
    from pybos import Callable, Caller
    
    bus = await Bus.create(BusConfig())
    
    # RPC handler: add two numbers
    def add_handler(request):
        parts = request.split(",")
        a = int(parts[0])
        b = int(parts[1])
        return str(a + b)
    
    callable_obj = await Callable.create(bus, "rpc/add")
    
    async def run_handler():
        try:
            await callable_obj.run(add_handler)
        except asyncio.CancelledError:
            pass
    
    handler_task = asyncio.create_task(run_handler())
    await asyncio.sleep(0.2)
    
    # Make RPC calls
    caller = await Caller.create(bus, "rpc/add")
    result1 = await caller.call_text("5,3")
    result2 = await caller.call_text("10,20")
    
    # Verify results
    assert result1 == "8"
    assert result2 == "30"
    
    handler_task.cancel()
    try:
        await handler_task
    except asyncio.CancelledError:
        pass
    
    print("✅ Callable.run() text handler test passed")


@pytest.mark.asyncio
async def test_callable_run_json_handler():
    """Test Callable.run_json() with JSON dict handler"""
    from pybos import Callable, Caller
    
    bus = await Bus.create(BusConfig())
    
    # RPC handler: multiply two numbers with JSON
    def multiply_handler(data):
        """Receives parsed JSON dict, returns dict"""
        a = data["a"]
        b = data["b"]
        return {"result": a * b, "op": "multiply"}
    
    callable_obj = await Callable.create(bus, "rpc/multiply")
    
    async def run_handler():
        try:
            await callable_obj.run_json(multiply_handler)
        except asyncio.CancelledError:
            pass
    
    handler_task = asyncio.create_task(run_handler())
    await asyncio.sleep(1.0)
    caller = await Caller.create(bus, "rpc/multiply")
    result1 = await caller.call_text(json.dumps({"a": 5, "b": 3}))
    result2 = await caller.call_text(json.dumps({"a": 10, "b": 20}))
    
    # Verify results (responses are JSON strings)
    assert json.loads(result1) == {"result": 15, "op": "multiply"}
    assert json.loads(result2) == {"result": 200, "op": "multiply"}
    
    handler_task.cancel()
    try:
        await handler_task
    except asyncio.CancelledError:
        pass
    
    print("✅ Callable.run_json() handler test passed")


@pytest.mark.asyncio
async def test_callable_sync_handler():
    """Test that sync handlers also work with Callable.run()"""
    from pybos import Callable, Caller

    bus = await Bus.create(BusConfig())

    # Sync handler (not async)
    call_count = [0]

    def sync_handler(request):
        """Synchronous handler"""
        call_count[0] += 1
        return request.upper()

    callable_srv = await Callable.create(bus, "test/sync")

    async def run_handler():
        try:
            await callable_srv.run(sync_handler)
        except asyncio.CancelledError:
            pass

    handler_task = asyncio.create_task(run_handler())
    await asyncio.sleep(1.0)
    caller = await Caller.create(bus, "test/sync")
    result = await caller.call_text("hello")

    assert result == "HELLO"
    assert call_count[0] == 1

    handler_task.cancel()
    try:
        await handler_task
    except asyncio.CancelledError:
        pass

    print("✅ Callable.run() sync handler test passed")
    """Test that sync handlers also work with Callable.run()"""
    from pybos import Callable, Caller
    
    bus = await Bus.create(BusConfig())
    
    def sync_handler(request):
        """Synchronous handler"""
        return f"processed: {request}"
    
    callable_obj = await Callable.create(bus, "rpc/sync")
    
    async def run_handler():
        try:
            await callable_obj.run(sync_handler)
        except asyncio.CancelledError:
            pass
    
    handler_task = asyncio.create_task(run_handler())
    await asyncio.sleep(1.0)
    caller = await Caller.create(bus, "rpc/sync")
    result = await caller.call_text("test")
    
    assert result == "processed: test"
    
    handler_task.cancel()
    try:
        await handler_task
    except asyncio.CancelledError:
        pass
    
    print("✅ Callable.run() sync handler test passed")


@pytest.mark.asyncio
async def test_multiple_callables():
    """Test multiple callables running simultaneously"""
    from pybos import Callable, Caller

    bus = await Bus.create(BusConfig())

    def echo_handler(request):
        return json.dumps({"echo": json.loads(request)})

    def upper_handler(request):
        return request.upper()

    c1 = await Callable.create(bus, "rpc/echo")
    c2 = await Callable.create(bus, "rpc/upper")

    async def run_echo():
        try:
            await c1.run(echo_handler)
        except asyncio.CancelledError:
            pass

    async def run_upper():
        try:
            await c2.run(upper_handler)
        except asyncio.CancelledError:
            pass

    task1 = asyncio.create_task(run_echo())
    task2 = asyncio.create_task(run_upper())

    await asyncio.sleep(1.0)

    caller1 = await Caller.create(bus, "rpc/echo")
    caller2 = await Caller.create(bus, "rpc/upper")

    resp1 = await caller1.call_text(json.dumps({"msg": "test"}))
    resp2 = await caller2.call_text("hello")

    assert json.loads(resp1) == {"echo": {"msg": "test"}}
    assert resp2 == "HELLO"

    task1.cancel()
    task2.cancel()
    for task in [task1, task2]:
        try:
            await task
        except asyncio.CancelledError:
            pass

    print("✅ Multiple callables test passed")
