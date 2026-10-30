"""A minimal MCP server for the BSH adapter test.

It speaks newline-delimited JSON-RPC 2.0 and implements initialize,
tools/list, tools/call, resources/list, resources/read, prompts/list and
prompts/get.
"""

import json
import sys


def result(request_id, value):
    return {"jsonrpc": "2.0", "id": request_id, "result": value}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = request.get("method")
        request_id = request.get("id")
        if method == "initialize":
            response = result(request_id, {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}, "resources": {}, "prompts": {}},
                "serverInfo": {"name": "bsh-mcp-echo", "version": "1.0.0"},
            })
        elif method == "notifications/initialized":
            continue
        elif method == "tools/list":
            response = result(request_id, {"tools": [{
                "name": "echo",
                "description": "Echo the arguments through MCP",
                "inputSchema": {"type": "object", "properties": {}},
            }]})
        elif method == "tools/call":
            arguments = request.get("params", {}).get("arguments", {})
            response = result(request_id, {
                "content": [{"type": "text", "text": json.dumps(arguments)}],
                "isError": False,
            })
        elif method == "resources/list":
            response = result(request_id, {"resources": [{
                "uri": "bsh://guide",
                "name": "guide",
                "description": "A guide from MCP",
                "mimeType": "text/plain",
            }]})
        elif method == "resources/read":
            response = result(request_id, {"contents": [{
                "uri": "bsh://guide",
                "text": "MCP resource text.",
            }]})
        elif method == "prompts/list":
            response = result(request_id, {"prompts": [{
                "name": "greet",
                "description": "A greeting prompt",
            }]})
        elif method == "prompts/get":
            response = result(request_id, {"messages": [{
                "role": "user",
                "content": {"type": "text", "text": "Say hello from MCP."},
            }]})
        else:
            response = {"jsonrpc": "2.0", "id": request_id, "error": {
                "code": -32601, "message": "method not found",
            }}
        print(json.dumps(response), flush=True)


if __name__ == "__main__":
    main()
