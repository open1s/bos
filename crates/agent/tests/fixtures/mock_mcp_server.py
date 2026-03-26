#!/usr/bin/env python3
import sys
import json
import uuid

def send_response(result, id):
    response = {
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    }
    print(json.dumps(response), flush=True)

def send_error(code, message, id):
    response = {
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    }
    print(json.dumps(response), flush=True)

def handle_initialize(request):
    send_response({
        "protocolVersion": "2025-03-26",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "mock-mcp-server",
            "version": "1.0.0"
        }
    }, request["id"])

def handle_tools_list(request):
    send_response({
        "tools": [
            {
                "name": "echo_tool",
                "description": "Echo a message",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "message": {"type": "string"}
                    },
                    "required": ["message"]
                }
            }
        ]
    }, request["id"])

def handle_tools_call(request):
    params = request.get("params", {})
    name = params.get("name")
    arguments = params.get("arguments", {})

    if name == "echo_tool":
        message = arguments.get("message", "")
        send_response({
            "content": [
                {
                    "type": "text",
                    "text": f"Echoed: {message}"
                }
            ]
        }, request["id"])
    else:
        send_error(-32601, f"Tool not found: {name}", request["id"])

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
                handle_tools_call(request)
            elif method.startswith("notifications/"):
                pass
            else:
                send_error(-32601, f"Method not found: {method}", req_id)

        except json.JSONDecodeError as e:
            send_error(-32700, f"Parse error: {e}", None)
        except Exception as e:
            send_error(-32603, f"Internal error: {e}", None)

if __name__ == "__main__":
    main()
