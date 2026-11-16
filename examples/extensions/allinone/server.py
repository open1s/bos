"""All-in-One Exam Demo: the BSH extension framework in one process package.

It answers the JSON-RPC handshake, implements seven tools (including progress
and a deliberate failure), handles panel-command actions, records every host
event it subscribed to, and reports the configuration the host passed.

    bsh ext validate examples/extensions/allinone
    bsh ext mock examples/extensions/allinone describe '{}'
    python3 examples/extensions/allinone/scripts/exam.py
"""

import json
import os
import sys
import time
import traceback

DEFAULTS = {"greeting": "hello from the all-in-one demo", "verbose": True}
ENV_KEYS = ("PYTHONDONTWRITEBYTECODE", "DEMO_GREETING")
TOOLS = ["describe", "echo", "sum", "progress", "fail", "events", "config"]

CONFIG = dict(DEFAULTS)
EVENTS = []
STARTED = time.time()


class ToolError(Exception):
    """A refusal that becomes a JSON-RPC error, and therefore a tool error."""


def emit(value):
    sys.stdout.write(json.dumps(value, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def progress(message, percent):
    # Notifications carry no id: the host forwards them as live tool progress.
    emit({"jsonrpc": "2.0", "method": "$/progress",
          "params": {"message": message, "percent": percent}})


def _number(value, name):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ToolError("%s must be a number" % name)
    return value


def _integer(value, name, default, low, high):
    if value is None:
        return default
    if isinstance(value, str):
        value = value.strip()
        if not value:
            # A workflow template that has no input renders as an empty string;
            # treat it as "not supplied" so the tool's default still applies.
            return default
        try:
            value = int(value)
        except ValueError:
            raise ToolError("%s must be an integer" % name)
    if isinstance(value, bool) or not isinstance(value, int):
        raise ToolError("%s must be an integer" % name)
    if value < low or value > high:
        raise ToolError("%s must be between %d and %d" % (name, low, high))
    return value


def describe(_arguments):
    return {
        "id": "com.example.allinone",
        "name": "All-in-One Exam Demo",
        "version": "1.0.0",
        "runtime": "process",
        "protocol": "jsonrpc-2.0",
        "tools": TOOLS,
        "surfaces": ["tool", "command", "panel", "prompt", "context", "skill",
                     "language", "integration", "theme", "preset",
                     "model_provider", "agent_provider", "workflow", "event",
                     "configuration"],
        "configuration": CONFIG,
        "environment": {key: os.environ.get(key) for key in ENV_KEYS},
        "events_received": len(EVENTS),
        "uptime_seconds": round(time.time() - STARTED, 3),
        "verbose": bool(CONFIG.get("verbose", True)),
    }


def echo(arguments):
    text = arguments.get("text")
    if not isinstance(text, str) or not text:
        raise ToolError("text must be a non-empty string")
    return {"text": text, "greeting": CONFIG.get("greeting"), "arguments": arguments}


def total(arguments):
    left = _number(arguments.get("a"), "a")
    right = _number(arguments.get("b"), "b")
    return {"a": left, "b": right, "sum": left + right}


def report_progress(arguments):
    steps = _integer(arguments.get("steps"), "steps", 3, 1, 10)
    started = time.time()
    for index in range(1, steps + 1):
        progress("step %d of %d" % (index, steps), round(index / steps, 3))
        time.sleep(0.05)
    return {"steps": steps, "completed": True,
            "duration_ms": round((time.time() - started) * 1000, 1)}


def fail(arguments):
    message = arguments.get("message")
    if not isinstance(message, str) or not message:
        message = "the all-in-one demo failed on purpose"
    raise ToolError(message)


def recent_events(arguments):
    limit = _integer(arguments.get("limit"), "limit", 5, 1, 50)
    topics = sorted({event["topic"] for event in EVENTS if event.get("topic")})
    return {"count": len(EVENTS), "topics": topics, "recent": EVENTS[-limit:]}


def configuration(_arguments):
    return {
        "configuration": CONFIG,
        "defaults": DEFAULTS,
        "environment": {key: os.environ.get(key) for key in ENV_KEYS},
    }


TOOL_HANDLERS = {
    "describe": describe,
    "echo": echo,
    "sum": total,
    "progress": report_progress,
    "fail": fail,
    "events": recent_events,
    "config": configuration,
}


def panel_document():
    """The UiDocument a panel action returns; the host applies revision + 1."""
    rows = [
        ["runtime", "process (python3)"],
        ["events", str(len(EVENTS))],
        ["greeting", str(CONFIG.get("greeting"))],
        ["refreshed", time.strftime("%H:%M:%S")],
    ]
    return {
        "schema_version": "1.0",
        "root": {
            "type": "column",
            "children": [
                {"type": "heading", "text": "All-in-One Exam Demo", "level": 2},
                {"type": "notification", "level": "info",
                 "text": "Refreshed by commands/execute at revision 2."},
                {"type": "text",
                 "text": "This document was returned by server.py, not read from disk."},
                {"type": "table", "columns": ["Key", "Value"], "rows": rows},
                {"type": "button", "label": "Refresh again", "action": "refresh",
                 "style": "primary"},
                {"type": "button", "label": "Say hello", "action": "hello"},
                {"type": "divider"},
                {"type": "markdown",
                 "text": "The panel dispatcher validated revision 2 and replaced the tree."},
            ],
        },
    }


def execute_command(name, arguments):
    if name == "refresh":
        return panel_document()
    if name == "hello":
        return {"inserted": "All-in-One demo says hello",
                "greeting": CONFIG.get("greeting"), "arguments": arguments}
    raise ToolError("unknown command %r" % name)


def handle(request):
    method = request.get("method")
    params = request.get("params") or {}
    request_id = request.get("id")
    if method == "initialize":
        raw = params.get("config")
        if isinstance(raw, str) and raw.strip():
            try:
                parsed = json.loads(raw)
                if isinstance(parsed, dict):
                    CONFIG.update(parsed)
            except ValueError:
                pass
        return {"result": {"protocolVersion": "1.0",
                            "token": params.get("token", ""),
                            "name": "allinone"}}, True
    if method == "shutdown":
        return {"result": {}}, False
    if method == "tools/call":
        name = str(params.get("name") or "").rsplit("__", 1)[-1]
        handler = TOOL_HANDLERS.get(name)
        if handler is None:
            return {"error": {"code": -32601, "message": "unknown tool %r" % name}}, True
        try:
            return {"result": handler(params.get("arguments") or {})}, True
        except ToolError as error:
            return {"error": {"code": -32000, "message": str(error)}}, True
        except Exception as error:  # a bug is reported, never a silent success
            traceback.print_exc(file=sys.stderr)
            return {"error": {"code": -32603,
                              "message": "%s: %s" % (type(error).__name__, error)}}, True
    if method == "commands/execute":
        try:
            return {"result": execute_command(str(params.get("name") or ""),
                                              params.get("arguments") or {})}, True
        except ToolError as error:
            return {"error": {"code": -32000, "message": str(error)}}, True
    if method == "events/event":
        event = params.get("event") or {}
        EVENTS.append({
            "topic": event.get("event_type"),
            "source": event.get("source"),
            "timestamp": event.get("timestamp"),
            "payload": event.get("payload"),
        })
        return None, True  # a notification has no response
    if method and method.startswith("$/progress"):
        return None, True
    return {"error": {"code": -32601, "message": "unsupported method %r" % method}}, True


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except ValueError:
            continue
        outcome, keep_going = handle(request)
        if outcome is not None:
            envelope = {"jsonrpc": "2.0", "id": request.get("id")}
            envelope.update(outcome)
            emit(envelope)
        if not keep_going:
            break


if __name__ == "__main__":
    main()
