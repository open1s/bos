"""Tool decorator and definition for brainos."""

from __future__ import annotations

import inspect
import json
from dataclasses import dataclass, field
from typing import Any, Callable


@dataclass
class ToolDef:
    name: str
    description: str
    callback: Callable[[dict], Any]
    parameters: dict[str, Any] = field(default_factory=dict)
    schema: dict[str, Any] = field(default_factory=dict)


class ToolResult:
    def __init__(self, success: bool, data: Any = None, error: str | None = None):
        self.success = success
        self.data = data
        self.error = error

    @staticmethod
    def success(data: Any) -> "ToolResult":
        return ToolResult(True, data)

    @staticmethod
    def error(message: str) -> "ToolResult":
        return ToolResult(False, None, message)


def tool(
    description: str,
    *,
    name: str | None = None,
    schema: dict[str, Any] | None = None,
) -> Callable[[Callable], ToolDef]:

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
            try:
                result = func(**kwargs)
                if isinstance(result, ToolResult):
                    if result.success:
                        return json.dumps(result.data) if result.data is not None else ""
                    else:
                        return json.dumps({"error": result.error})
                if isinstance(result, str):
                    return result
                return json.dumps(result)
            except Exception as e:
                return json.dumps({"error": str(e)})

        return ToolDef(
            name=tool_name,
            description=description,
            callback=wrapper,
            parameters=params.get("properties", {}),
            schema=schema or _build_schema(params),
        )

    return decorator


def _extract_params(func: Callable) -> dict[str, Any]:
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

        prop = {"type": json_type}
        if param.default is not inspect.Parameter.empty:
            prop["default"] = param.default
        else:
            required.append(param_name)

        properties[param_name] = prop

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
    if "type" in params:
        return params
    return {"type": "object", "properties": params}