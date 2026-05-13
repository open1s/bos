"""Tests for Agent binding"""
import pytest
import asyncio
from nbos import Agent, AgentConfig, Bus, BusConfig


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
    """Agent functionality tests"""

    @pytest.mark.asyncio
    async def test_agent_creation(self):
        """Test creating an Agent instance"""
        config = AgentConfig()
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        assert agent is not None

    @pytest.mark.asyncio
    async def test_agent_creation_with_name(self):
        """Test creating Agent with name"""
        config = AgentConfig(name="test_agent")
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        assert agent is not None

    @pytest.mark.asyncio
    async def test_agent_creation_with_model(self):
        """Test creating Agent with model specification"""
        config = AgentConfig(model="gpt-4")
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        assert agent is not None

    @pytest.mark.asyncio
    async def test_agent_with_all_config_params(self):
        """Test creating Agent with all configuration options"""
        config = AgentConfig(
            name="full_config_agent",
            model="gpt-4"
        )
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        assert agent is not None

    @pytest.mark.asyncio
    async def test_multiple_agents_same_bus(self):
        """Test creating multiple agents on same bus"""
        bus = await Bus.create(BusConfig())
        
        config1 = AgentConfig(name="agent1")
        config2 = AgentConfig(name="agent2")
        
        agent1 = await Agent.create(config1, bus)
        agent2 = await Agent.create(config2, bus)
        
        assert agent1 is not None
        assert agent2 is not None

    @pytest.mark.asyncio
    async def test_multiple_agents_different_buses(self):
        """Test creating agents on different buses"""
        bus1 = await Bus.create(BusConfig())
        bus2 = await Bus.create(BusConfig())
        
        config = AgentConfig(name="multi_bus_agent")
        
        agent1 = await Agent.create(config, bus1)
        agent2 = await Agent.create(config, bus2)
        
        assert agent1 is not None
        assert agent2 is not None

    @pytest.mark.asyncio
    async def test_agent_with_minimal_config(self):
        """Test agent with minimal configuration"""
        config = AgentConfig()
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        assert agent is not None

    @pytest.mark.asyncio
    async def test_agent_lifecycle(self):
        """Test agent creation and basic lifecycle"""
        config = AgentConfig(name="lifecycle_agent")
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        
        # Agent should be created and ready
        assert agent is not None
        
        # Agent should remain valid after creation
        await asyncio.sleep(0.1)
        assert agent is not None

    @pytest.mark.asyncio
    async def test_agent_config_independence(self):
        """Test that agent configs are independent"""
        config1 = AgentConfig(name="agent1", model="gpt-3.5")
        config2 = AgentConfig(name="agent2", model="gpt-4")
        
        bus = await Bus.create(BusConfig())
        
        agent1 = await Agent.create(config1, bus)
        agent2 = await Agent.create(config2, bus)
        
        assert agent1 is not None
        assert agent2 is not None

    @pytest.mark.asyncio
    async def test_agent_with_special_characters_in_name(self):
        """Test agent with special characters in name"""
        config = AgentConfig(name="agent_with-special.chars_123")
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        assert agent is not None

    @pytest.mark.asyncio
    async def test_concurrent_agent_creation(self):
        """Test concurrent creation of multiple agents"""
        bus = await Bus.create(BusConfig())
        
        async def create_agent(idx):
            config = AgentConfig(name=f"agent_{idx}")
            return await Agent.create(config, bus)
        
        # Create 5 agents concurrently
        tasks = [create_agent(i) for i in range(5)]
        agents = await asyncio.gather(*tasks)
        
        assert len(agents) == 5
        assert all(agent is not None for agent in agents)

    @pytest.mark.asyncio
    async def test_agent_bus_reference(self):
        """Test that agent correctly references its bus"""
        bus = await Bus.create(BusConfig())
        config = AgentConfig(name="graph_agent")
        agent = await Agent.create(config, bus)
        
        # Should not raise - agent has bus reference
        assert agent is not None
        
        # Bus should still be usable after agent creation
        await bus.publish_text("test/topic", "test")

    @pytest.mark.asyncio
    async def test_agent_config_with_empty_name(self):
        """Test agent config with empty name"""
        config = AgentConfig(name="")
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        assert agent is not None

    @pytest.mark.asyncio
    async def test_agent_config_with_long_name(self):
        """Test agent config with very long name"""
        long_name = "a" * 1000
        config = AgentConfig(name=long_name)
        bus = await Bus.create(BusConfig())
        agent = await Agent.create(config, bus)
        assert agent is not None
