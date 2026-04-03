#!/usr/bin/env python3
"""
Minimal MCP server with Streamable HTTP transport for testing.

Usage:
    python3 crates/examples/mcp_http_server.py
    # Server starts on http://localhost:8000/mcp
"""

import json
import time
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse
import threading
import uuid

class McpHttpHandler(BaseHTTPRequestHandler):
    session_id = None

    def log_message(self, format, *args):
        pass

    def do_POST(self):
        content_length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(content_length)

        try:
            req = json.loads(body)
        except json.JSONDecodeError:
            self._send_error(-32700, "Parse error")
            return

        method = req.get("method", "")
        params = req.get("params", {})
        req_id = req.get("id")

        result = self._dispatch(method, params)

        if req_id is not None:
            resp = {"jsonrpc": "2.0", "id": req_id, "result": result}
            self._send_json(200, resp)
        else:
            self.send_response(202)
            self.end_headers()

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(b"MCP server running")

    def do_DELETE(self):
        McpHttpHandler.session_id = None
        self.send_response(200)
        self.end_headers()

    def _dispatch(self, method, params):
        if method == "initialize":
            McpHttpHandler.session_id = str(uuid.uuid4())
            return {
                "protocolVersion": "2025-03-26",
                "capabilities": {
                    "tools": {"listChanged": True},
                    "resources": {"listChanged": False},
                    "prompts": {"listChanged": False},
                },
                "serverInfo": {"name": "demo-http-server", "version": "1.0.0"},
            }
        elif method == "tools/list":
            return {
                "tools": [
                    {
                        "name": "greet",
                        "description": "Greet someone by name",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string", "description": "Person's name"}
                            },
                            "required": ["name"],
                        },
                    },
                    {
                        "name": "calc",
                        "description": "Calculate a math expression",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "a": {"type": "number"},
                                "b": {"type": "number"},
                                "op": {"type": "string", "enum": ["add", "sub", "mul", "div"]},
                            },
                            "required": ["a", "b", "op"],
                        },
                    },
                    {
                        "name": "time",
                        "description": "Get current UTC timestamp",
                        "inputSchema": {"type": "object", "properties": {}},
                    },
                ]
            }
        elif method == "tools/call":
            name = params.get("name", "")
            args = params.get("arguments", {})
            if name == "greet":
                return {"content": [{"type": "text", "text": f"Hello, {args.get('name', 'World')}!"}]}
            elif name == "calc":
                a, b = args.get("a", 0), args.get("b", 0)
                op = args.get("op", "add")
                ops = {"add": a + b, "sub": a - b, "mul": a * b, "div": a / b if b else "error"}
                result = ops.get(op, "unknown")
                return {"content": [{"type": "text", "text": str(result)}]}
            elif name == "time":
                return {"content": [{"type": "text", "text": str(int(time.time()))}]}
            return {"content": [{"type": "text", "text": f"Unknown tool: {name}"}], "isError": True}
        elif method == "notifications/initialized":
            return {}
        return {"error": f"Method not found: {method}"}

    def _send_json(self, code, data):
        body = json.dumps(data).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        if McpHttpHandler.session_id:
            self.send_header("Mcp-Session-Id", McpHttpHandler.session_id)
        self.end_headers()
        self.wfile.write(body)

    def _send_error(self, code, message):
        self._send_json(200, {"jsonrpc": "2.0", "error": {"code": code, "message": message}})


def run_server(port=8000):
    server = HTTPServer(("127.0.0.1", port), McpHttpHandler)
    print(f"  🚀 MCP HTTP server on http://127.0.0.1:{port}/mcp")
    server.serve_forever()


if __name__ == "__main__":
    run_server()
