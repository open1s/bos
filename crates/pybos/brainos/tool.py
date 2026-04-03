"""Tool decorator and definition for brainos.

Usage:
    from brainos import tool

    @tool("Calculate a math expression")
    def calc(expression: str) -> float:
        return eval(expression)

    # Or with explicit schema:
    @tool("Get weather", schema={...})
    def weather(city: str) -> dict:
        return {"city": city, "temp": 22}
"""

from __future__ import annotations

import inspect
import json
from dataclasses import dataclass, field
from typing import Any, Callable


@dataclass
class ToolDef:
    """A tool definition — created by @tool() or manually."""

    name: str
    description: str
    callback: Callable[[dict], Any]
    parameters: dict[str, Any] = field(default_factory=dict)
    schema: dict[str, Any] = field(default_factory=dict)


def tool(
    description: str,
    *,
    name: str | None = None,
    schema: dict[str, Any] | None = None,
) -> Callable[[Callable], ToolDef]:
    """Decorator that turns a function into a BrainOS tool.

    Args:
        description: Human-readable description of what the tool does.
        name: Tool name (defaults to function name).
        schema: JSON Schema for parameters (auto-generated from signature if omitted).

    Returns:
        A ToolDef that can be registered with Agent.register().

    Example:
        @tool("Add two numbers together")
        def add(a: int, b: int) -> int:
            return a + b
    """

    def decorator(func: Callable) -> ToolDef:
        tool_name = name or func.__name__
        params = _extract_params(func) if schema is None else schema

        def wrapper(args: dict) -> str:
            kwargs = {}
            properties = params.get("properties", {})
            for k, spec in properties.items():
                value = args.get(k)
                if value is not None:
                    json_type = spec.get("type", "string")
                    kwargs[k] = _coerce_type(value, json_type)
            result = func(**kwargs)
            if isinstance(result, str):
                return result
            return json.dumps(result)

        return ToolDef(
            name=tool_name,
            description=description,
            callback=wrapper,
            parameters=params.get("properties", {}),
            schema=schema or _build_schema(params),
        )

    return decorator


def _extract_params(func: Callable) -> dict[str, Any]:
    """Extract JSON Schema from function signature."""
    sig = inspect.signature(func)
    properties: dict[str, Any] = {}
    required: list[str] = []

    type_map = {
        int: "integer",
        float: "number",
        str: "string",
        bool: "boolean",
        list: "array",
        dict: "object",
    }

    for param_name, param in sig.parameters.items():
        if param_name in ("self", "cls"):
            continue

        param_type = param.annotation
        json_type = type_map.get(param_type, "string")

        properties[param_name] = {"type": json_type}

        if param.default is inspect.Parameter.empty:
            required.append(param_name)
        else:
            properties[param_name]["default"] = param.default

    return {"type": "object", "properties": properties, "required": required}


def _coerce_type(value: Any, json_type: str) -> Any:
    if json_type == "integer":
        if isinstance(value, str):
            return int(value)
        return int(value)
    elif json_type == "number":
        if isinstance(value, str):
            return float(value)
        return float(value)
    elif json_type == "boolean":
        if isinstance(value, str):
            return value.lower() in ("true", "1", "yes")
        return bool(value)
    return value


def _build_schema(params: dict) -> dict[str, Any]:
    """Wrap parameters dict into full JSON Schema."""
    if "type" in params:
        return params
    return {"type": "object", "properties": params}
