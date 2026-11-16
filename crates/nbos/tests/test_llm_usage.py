"""Tests for LlmUsage binding"""
import pytest
from nbos import LlmUsage, PromptTokensDetails


class TestLlmUsage:
    """LlmUsage functionality tests"""

    def test_create_llm_usage(self):
        """Test creating an LlmUsage instance"""
        usage = LlmUsage(
            prompt_tokens=100,
            completion_tokens=50,
            total_tokens=150,
        )
        assert usage.prompt_tokens == 100
        assert usage.completion_tokens == 50
        assert usage.total_tokens == 150

    def test_llm_usage_with_details(self):
        """Test creating LlmUsage with prompt tokens details"""
        details = PromptTokensDetails(audio_tokens=10, cached_tokens=20)
        usage = LlmUsage(
            prompt_tokens=100,
            completion_tokens=50,
            total_tokens=150,
        )
        usage.prompt_tokens_details = details
        assert usage.prompt_tokens_details is not None
        assert usage.prompt_tokens_details.audio_tokens == 10
        assert usage.prompt_tokens_details.cached_tokens == 20

    def test_llm_usage_with_none_details(self):
        """Test creating LlmUsage without details"""
        usage = LlmUsage(
            prompt_tokens=100,
            completion_tokens=50,
            total_tokens=150,
        )
        assert usage.prompt_tokens_details is None

    def test_llm_usage_zero_values(self):
        """Test creating LlmUsage with zero values"""
        usage = LlmUsage(
            prompt_tokens=0,
            completion_tokens=0,
            total_tokens=0,
        )
        assert usage.prompt_tokens == 0
        assert usage.completion_tokens == 0
        assert usage.total_tokens == 0

    def test_llm_usage_max_values(self):
        """Test creating LlmUsage with max values"""
        usage = LlmUsage(
            prompt_tokens=2147483647,
            completion_tokens=2147483647,
            total_tokens=4294967294,
        )
        assert usage.prompt_tokens == 2147483647
        assert usage.completion_tokens == 2147483647
        assert usage.total_tokens == 4294967294


class TestPromptTokensDetails:
    """PromptTokensDetails functionality tests"""

    def test_create_details(self):
        """Test creating a PromptTokensDetails instance"""
        details = PromptTokensDetails(audio_tokens=5, cached_tokens=10)
        assert details.audio_tokens == 5
        assert details.cached_tokens == 10

    def test_create_details_with_none(self):
        """Test creating details with None values"""
        details = PromptTokensDetails(audio_tokens=None, cached_tokens=None)
        assert details.audio_tokens is None
        assert details.cached_tokens is None

    def test_create_only_audio_tokens(self):
        """Test creating details with only audio tokens"""
        details = PromptTokensDetails(audio_tokens=5, cached_tokens=None)
        assert details.audio_tokens == 5
        assert details.cached_tokens is None

    def test_create_only_cached_tokens(self):
        """Test creating details with only cached tokens"""
        details = PromptTokensDetails(audio_tokens=None, cached_tokens=10)
        assert details.audio_tokens is None
        assert details.cached_tokens == 10