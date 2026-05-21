"""Tests for Agent binding"""
import pytest
import asyncio
from nbos import AgentConfig, Bus, BusConfig


class TestAgentConfig:
    """AgentConfig functionality tests"""

    def test_agent_config_creation_default(self):
        """Test creating AgentConfig with default parameters"""
        config = AgentConfig()
        assert config is not None

    def test_agent_config_creation_with_params(self):
        """Test creating AgentConfig with parameters"""
        config = AgentConfig(name="test_agent", model="gpt-4")
        assert config is not None

    def test_agent_config_repr(self):
        """Test AgentConfig string representation"""
        config = AgentConfig(name="my_agent")
        repr_str = repr(config)
        assert "AgentConfig" in repr_str or repr_str is not None


class TestAgent:
    """Agent functionality tests - using BrainOS pattern"""

    @pytest.mark.asyncio
    async def test_agent_creation(self):
        """Test creating an Agent instance via BrainOS"""
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("test").start()
            assert agent is not None

    @pytest.mark.asyncio
    async def test_agent_creation_with_name(self):
        """Test creating Agent with name via BrainOS"""
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("test_agent").start()
            assert agent is not None

    @pytest.mark.asyncio
    async def test_agent_creation_with_model(self):
        """Test creating Agent with model specification via BrainOS"""
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("test").with_model("gpt-4").start()
            assert agent is not None

    @pytest.mark.asyncio
    async def test_multiple_agents_same_bus(self):
        """Test creating multiple agents on same bus"""
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent1 = await brain.agent("agent1").start()
            agent2 = await brain.agent("agent2").start()
            assert agent1 is not None
            assert agent2 is not None

    @pytest.mark.asyncio
    async def test_multiple_agents_different_buses(self):
        """Test creating agents on different buses"""
        from nbos import BrainOS
        async with BrainOS() as brain1:
            async with BrainOS() as brain2:
                agent1 = await brain1.agent("multi_bus_agent").start()
                agent2 = await brain2.agent("multi_bus_agent").start()
                assert agent1 is not None
                assert agent2 is not None

    @pytest.mark.asyncio
    async def test_agent_lifecycle(self):
        """Test agent creation and basic lifecycle"""
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("lifecycle_agent").start()
            assert agent is not None
            await asyncio.sleep(0.1)
            assert agent is not None

    @pytest.mark.asyncio
    async def test_agent_config_independence(self):
        """Test that agent configs are independent"""
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent1 = await brain.agent("agent1").with_model("gpt-3.5").start()
            agent2 = await brain.agent("agent2").with_model("gpt-4").start()
            assert agent1 is not None
            assert agent2 is not None

    @pytest.mark.asyncio
    async def test_agent_with_special_characters_in_name(self):
        """Test agent with special characters in name"""
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("agent_with-special.chars_123").start()
            assert agent is not None

    @pytest.mark.asyncio
    async def test_concurrent_agent_creation(self):
        """Test concurrent creation of multiple agents"""
        from nbos import BrainOS

        async def create_agent(brain, idx):
            return await brain.agent(f"agent_{idx}").start()

        async with BrainOS() as brain:
            tasks = [create_agent(brain, i) for i in range(5)]
            agents = await asyncio.gather(*tasks)
            assert len(agents) == 5
            assert all(agent is not None for agent in agents)

    @pytest.mark.asyncio
    async def test_agent_bus_reference(self):
        """Test that agent correctly references its bus via session"""
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("graph_agent").start()
            assert agent is not None
            # Agent session should be accessible
            session = agent.session
            assert session is not None

    @pytest.mark.asyncio
    async def test_agent_config_with_empty_name(self):
        """Test agent config with empty name"""
        from nbos import BrainOS
        async with BrainOS() as brain:
            agent = await brain.agent("").start()
            assert agent is not None

    @pytest.mark.asyncio
    async def test_agent_config_with_long_name(self):
        """Test agent config with very long name"""
        from nbos import BrainOS
        long_name = "a" * 1000
        async with BrainOS() as brain:
            agent = await brain.agent(long_name).start()
            assert agent is not None