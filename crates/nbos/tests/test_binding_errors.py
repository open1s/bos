"""Guards for input validation at the Python/native boundary.

Malformed input must raise a Python exception; silently substituting a default
hides the mistake from the caller (and, for a tool schema, from the model).
"""

import pytest

from nbos import HookEvent, HookRegistry
from nbos_native import ConfigLoader, PythonTool


def _noop(_args):
    return "{}"


def test_python_tool_rejects_malformed_schema_json():
    with pytest.raises(ValueError, match="invalid tool schema JSON"):
        PythonTool("bad", "bad schema", "{}", "{not json", _noop)


def test_hook_registry_rejects_an_unknown_event():
    registry = HookRegistry()
    with pytest.raises(ValueError, match="Unknown hook event"):
        registry.register(HookEvent("OnMesage"), lambda event, ctx: None)


def test_hook_registry_accepts_a_known_event():
    registry = HookRegistry()
    registry.register(HookEvent("OnMessage"), lambda event, ctx: None)


def test_json_conversion_rejects_cyclic_containers():
    # A cycle used to recurse until the Rust stack overflowed and aborted the
    # interpreter.
    cyclic: list = []
    cyclic.append(cyclic)

    loader = ConfigLoader()
    with pytest.raises(ValueError, match="nested more than"):
        loader.add_inline(cyclic)
