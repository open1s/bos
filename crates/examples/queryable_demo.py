import asyncio
import json
from pybrainos import Bus, BusConfig

async def test_callable_run_text_handler():
    """Test Callable.run() with text handler"""
    from pybrainos import Queryable, Query
    
    bus = await Bus.create(BusConfig())
    
    # Handler that echoes back the request
    def handler(request):
        data = json.loads(request)
        data['echo'] = True
        return json.dumps(data)
    
    # Create callable with run handler
    queryable = await Queryable.create(bus, "test/echo")
    
    # Start handler in background - wrap in async function for create_task
    async def run_handler():
        try: 
            await queryable.run(handler)
        except asyncio.CancelledError:
            pass
    
    handler_task = asyncio.create_task(run_handler())
    
    # Give handler time to start and register with Zenoh
    await asyncio.sleep(0.1)
    
    # Make a call from another client
    query = await Query.create(bus, "test/echo")
    print(query.topic())
    response = await query.query_text(json.dumps({"msg": "hello"}))
    
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
    
    print("✅ Queryable.run() text handler test passed")

if __name__ == "__main__":
    asyncio.run(test_callable_run_text_handler())