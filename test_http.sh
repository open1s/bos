#!/bin/bash
set -e

cd /Users/gaosg/Projects/bos

# Start Python HTTP server in background
python3 -c "
import sys
import json
import threading
import time
from http.server import HTTPServer, BaseHTTPRequestHandler

port = 9898

class LoggingHandler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass
    
    def do_POST(self):
        content_length = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(content_length)
        print('[PY] Received POST:', body[:100], file=__import__('sys').stderr, flush=True)
        
        resp = {'jsonrpc': '2.0', 'id': 1, 'result': {'ok': True}}
        resp_body = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(resp_body)))
        self.end_headers()
        self.wfile.write(resp_body)

server = HTTPServer(('127.0.0.1', port), LoggingHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
print('[PY] Server started on port', port, file=__import__('sys').stderr, flush=True)
time.sleep(10)
" 2>&1 &

PY_PID=$!
sleep 2

# Test with Rust
echo "Testing with Rust reqwest..."
cargo run --manifest-path=/dev/stdin 2>&1 <<'EOF'
[package]
name = "test_reqwest"
version = "0.1.0"
edition = "2021"

[dependencies]
reqwest = { version = "0.12", features = ["json"] }
serde_json = "1.0"
tokio = { version = "1", features = ["full"] }

[[bin]]
name = "test"
path = "/Users/gaosg/Projects/bos/test_reqwest.rs"
EOF

kill $PY_PID 2>/dev/null || true
