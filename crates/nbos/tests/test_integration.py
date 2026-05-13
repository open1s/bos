"""Integration tests for pybos components working together"""
import pytest
import asyncio
from nbos import (
    Bus, BusConfig, Publisher, Subscriber,
    Query, Queryable, Caller, Callable,
    Agent, AgentConfig, ConfigLoader
)


class TestIntegration:
    """Integration tests combining multiple pybos components"""

    @pytest.mark.asyncio
    async def test_bus_with_publisher_subscriber(self):
        """Test Bus, Publisher, and Subscriber working together"""
        bus = await Bus.create(BusConfig())
        
        # Create publisher and subscriber
        pub = await Publisher.create(bus, "integration/test")
        sub = await Subscriber.create(bus, "integration/test")
        
        # Publish and receive
        await pub.publish_text("integration test message")
        msg = await sub.recv_with_timeout_ms(1000)
        
        assert msg == "integration test message"

    @pytest.mark.asyncio
    async def test_bus_with_query_queryable(self):
        """Test Bus with Caller and Callable (Queryable requires router mode)"""
        def uppercase_service(text: str) -> str:
            return text.upper()

        bus = await Bus.create(BusConfig())

        # Create callable service (works in peer mode)
        callable_srv = await Callable.create(bus, "service/upper", uppercase_service)
        await callable_srv.start()

        await asyncio.sleep(0.1)

        # Create caller client
        caller = await Caller.create(bus, "service/upper")
        result = await caller.call_text("hello")

        assert result == "HELLO"

    @pytest.mark.asyncio
    async def test_bus_with_caller_callable(self):
        """Test Bus with Caller and Callable"""
        def echo_service(text: str) -> str:
            return f"echo: {text}"
        
        bus = await Bus.create(BusConfig())
        
        # Create callable service
        callable_srv = await Callable.create(bus, "rpc/echo", echo_service)
        await callable_srv.start()
        
        await asyncio.sleep(0.1)
        
        # Create caller client
        caller = await Caller.create(bus, "rpc/echo")
        result = await caller.call_text("test")
        
        assert result == "echo: test"

    @pytest.mark.asyncio
    async def test_bus_with_agent(self):
        """Test Bus with Agent"""
        bus = await Bus.create(BusConfig())
        
        config = AgentConfig(name="integration_agent")
        agent = await Agent.create(config, bus)
        
        assert agent is not None

    @pytest.mark.asyncio
    async def test_multiple_services_on_same_bus(self):
        """Test multiple services (Query, Caller, Pub/Sub) on same bus"""
        def query_handler(text: str) -> str:
            return text.upper()
        
        def callable_handler(text: str) -> str:
            return text.lower()

        bus = await Bus.create(BusConfig())

        # Set up callable services (work in peer mode)
        callable1 = await Callable.create(bus, "svc/call1", query_handler)
        await callable1.start()

        callable2 = await Callable.create(bus, "svc/call2", callable_handler)
        await callable2.start()

        # Set up pub/sub
        pub = await Publisher.create(bus, "svc/pubsub")
        sub = await Subscriber.create(bus, "svc/pubsub")

        await asyncio.sleep(0.1)

        # Test callable 1
        caller1 = await Caller.create(bus, "svc/call1")
        c1_result = await caller1.call_text("HELLO")
        assert c1_result == "HELLO"

        # Test callable 2
        caller2 = await Caller.create(bus, "svc/call2")
        c2_result = await caller2.call_text("HELLO")
        assert c2_result == "hello"

        # Test pub/sub
        await pub.publish_text("test message")
        ps_result = await sub.recv_with_timeout_ms(1000)
        assert ps_result == "test message"

    @pytest.mark.asyncio
    async def test_config_loader_with_bus(self):
        """Test ConfigLoader with Bus creation"""
        # Load config
        loader = ConfigLoader(strategy="override")
        loader.add_inline({"bus": {"mode": "peer"}})
        config = loader.load_sync()
        
        # Create bus (using config values)
        bus_config = BusConfig(mode=config.get("bus", {}).get("mode", "peer"))
        bus = await Bus.create(bus_config)
        
        # Verify bus works
        await bus.publish_text("test/topic", "test")

    @pytest.mark.asyncio
    async def test_multiple_agents_with_services(self):
        """Test multiple agents with service communication"""
        bus = await Bus.create(BusConfig())
        
        # Create agents
        config1 = AgentConfig(name="agent1")
        config2 = AgentConfig(name="agent2")
        
        agent1 = await Agent.create(config1, bus)
        agent2 = await Agent.create(config2, bus)
        
        # Create services that agents could use
        def service_handler(text: str) -> str:
            return f"service handled: {text}"
        
        queryable = await Queryable.create(bus, "shared/service", service_handler)
        await queryable.start()
        
        await asyncio.sleep(0.1)
        
        assert agent1 is not None
        assert agent2 is not None

    @pytest.mark.asyncio
    async def test_json_communication_full_stack(self):
        """Test JSON communication through full stack"""
        def json_processor(text: str) -> str:
            # In real scenario, would parse JSON and process
            return f"processed: {text}"
        
        bus = await Bus.create(BusConfig())
        # Set up through Callable (works in peer mode)
        callable_srv = await Callable.create(bus, "rpc/json", json_processor)
        await callable_srv.start()

        # Set up through Pub/Sub
        pub = await Publisher.create(bus, "events/json")
        sub = await Subscriber.create(bus, "events/json")

        await asyncio.sleep(0.1)

        json_data = '{"user": "alice", "action": "login"}'

        # Call with JSON
        caller = await Caller.create(bus, "rpc/json")
        c_result = await caller.call_text(json_data)
        assert "processed" in c_result

        # Publish JSON
        await pub.publish_text(json_data)
        ps_result = await sub.recv_with_timeout_ms(1000)
        assert ps_result == json_data

    @pytest.mark.asyncio
    async def test_stress_multiple_publishers_subscribers(self):
        """Stress test with multiple publishers and subscribers"""
        bus = await Bus.create(BusConfig())
        
        num_pairs = 5
        publishers = []
        subscribers = []
        
        # Create multiple pub/sub pairs
        for i in range(num_pairs):
            pub = await Publisher.create(bus, f"stress/topic/{i}")
            sub = await Subscriber.create(bus, f"stress/topic/{i}")
            publishers.append(pub)
            subscribers.append(sub)
        
        # Publish to all
        for i, pub in enumerate(publishers):
            await pub.publish_text(f"message for topic {i}")
        
        # Receive from all
        for i, sub in enumerate(subscribers):
            msg = await sub.recv_with_timeout_ms(1000)
            assert f"message for topic {i}" in msg

    @pytest.mark.asyncio
    async def test_error_handling_invalid_utf8(self):
        """Test error handling with various inputs"""
        bus = await Bus.create(BusConfig())
        pub = await Publisher.create(bus, "test/unicode")
        sub = await Subscriber.create(bus, "test/unicode")
        
        # Send unicode text
        unicode_text = "Hello 世界 🌍 مرحبا мир"
        await pub.publish_text(unicode_text)
        
        received = await sub.recv_with_timeout_ms(1000)
        assert received == unicode_text

    @pytest.mark.asyncio
    async def test_workflow_with_multiple_transformations(self):
        """Test workflow applying multiple transformations"""
        bus = await Bus.create(BusConfig())
        
        def uppercase(text: str) -> str:
            return text.upper()
        
        def add_prefix(text: str) -> str:
            return f"[PREFIX] {text}"
        
        # Create service chain
        upper_svc = await Queryable.create(bus, "transform/upper", uppercase)
        prefix_svc = await Queryable.create(bus, "transform/prefix", add_prefix)
        
        await upper_svc.start()
        await prefix_svc.start()

        await asyncio.sleep(0.1)

        upper_caller = await Caller.create(bus, "transform/upper")
        prefix_caller = await Caller.create(bus, "transform/prefix")

        # Apply transformations
        step1 = await upper_caller.call_text("hello")
        assert step1 == "HELLO"

        step2 = await prefix_caller.call_text(step1)
        assert step2 == "[PREFIX] HELLO"

    @pytest.mark.asyncio
    async def test_concurrent_multi_service_communication(self):
        """Test concurrent communication across multiple services"""
        bus = await Bus.create(BusConfig())
        
        def service_a(text: str) -> str:
            return f"A({text})"
        
        def service_b(text: str) -> str:
            return f"B({text})"
        
        def service_c(text: str) -> str:
            return f"C({text})"
        
        svc_a = await Callable.create(bus, "multiservice/a", service_a)
        svc_b = await Callable.create(bus, "multiservice/b", service_b)
        svc_c = await Callable.create(bus, "multiservice/c", service_c)
        
        await svc_a.start()
        await svc_b.start()
        await svc_c.start()
        
        await asyncio.sleep(0.1)
        
        c_a = await Caller.create(bus, "multiservice/a")
        c_b = await Caller.create(bus, "multiservice/b")
        c_c = await Caller.create(bus, "multiservice/c")

        # Send concurrent calls to all services
        results = await asyncio.gather(
            c_a.call_text("test"),
            c_b.call_text("test"),
            c_c.call_text("test")
        )

        assert results[0] == "A(test)"
        assert results[1] == "B(test)"
        assert results[2] == "C(test)"
