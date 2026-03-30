#!/usr/bin/env python3
import json
import sys


def send_response(result, request_id):
    print(
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": result,
            }
        ),
        flush=True,
    )


def send_error(code, message, request_id):
    print(
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": code, "message": message},
            }
        ),
        flush=True,
    )


def handle_initialize(request):
    send_response(
        {
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "mock-mcp-duplicate-server", "version": "1.0.0"},
        },
        request["id"],
    )


def handle_tools_list(request):
    duplicate_tool = {
        "name": "dup_tool",
        "description": "Duplicate test tool",
        "inputSchema": {
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
        },
    }
    send_response({"tools": [duplicate_tool, duplicate_tool]}, request["id"])


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
            method = request.get("method")
            req_id = request.get("id", 0)
            if method == "initialize":
                handle_initialize(request)
            elif method == "tools/list":
                handle_tools_list(request)
            elif method == "tools/call":
                send_response({"content": [{"type": "text", "text": "ok"}]}, req_id)
            elif method.startswith("notifications/"):
                pass
            else:
                send_error(-32601, f"Method not found: {method}", req_id)
        except Exception as e:
            send_error(-32603, f"Internal error: {e}", None)


if __name__ == "__main__":
    main()
