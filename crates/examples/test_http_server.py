#!/usr/bin/env python3
"""Test the HTTP MCP server"""

import sys
sys.path.insert(0, ".")
from mcp_http_server import run_server
import threading
import time
import json

# Start server in background thread
print("[TEST] Starting server on port 9999...")
server_thread = threading.Thread(target=lambda: run_server(9999), daemon=True)
server_thread.start()
time.sleep(1)

# Test with requests
try:
    import requests
    print("[TEST] Testing HTTP POST to server...")
    resp = requests.post(
        "http://127.0.0.1:9999/mcp",
        json={
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        },
        timeout=5
    )
    print(f"[TEST] Status: {resp.status_code}")
    print(f"[TEST] Headers: {dict(resp.headers)}")
    print(f"[TEST] Body: {resp.text}")
    if resp.status_code == 200:
        try:
            data = resp.json()
            print(f"[TEST] JSON: {json.dumps(data, indent=2)}")
        except:
            print(f"[TEST] Could not parse JSON")
except Exception as e:
    print(f"[TEST] Error: {e}")
    import traceback
    traceback.print_exc()
