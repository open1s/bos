"""Tests for pybos logging module."""

import pytest
from nbos import init_tracing


def test_init_tracing():
    """Test that init_tracing can be called without errors."""
    init_tracing()


def test_init_tracing_idempotent():
    """Test that init_tracing can be called multiple times."""
    init_tracing()
    init_tracing()