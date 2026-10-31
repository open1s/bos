"""Hook Demo: demonstrate every host hook point from a process extension.

The server answers the BSH JSON-RPC handshake, records every events/event
notification, and exposes the log as the hook-log tool. Run it with:

    bsh ext mock examples/extensions/hook-demo hook-log '{}'
"""

import json
import sys

EVENTS = []


def main() -> None:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = request.get("method")
        params = request.get("params", {})
        request_id = request.get("id")
        if method == "initialize":
            result = {
                "protocolVersion": "1.0",
                "token": params.get("token"),
                "name": "hook-demo",
            }
        elif method == "tools/call":
            arguments = params.get("arguments", {})
            name = params.get("name")
            if name == "hook-log":
                result = {
                    "count": len(EVENTS),
                    "events": EVENTS[-100:],
                    "arguments": arguments,
                }
            else:
                result = {"error": f"unknown tool {name}"}
        elif method == "events/event":
            event = params.get("event", {})
            EVENTS.append({
                "topic": event.get("event_type"),
                "source": event.get("source"),
                "payload": event.get("payload"),
                "at": event.get("timestamp"),
            })
            # Events are notifications, so no response is written.
            continue
        elif method == "shutdown":
            result = {}
        else:
            result = {}
        print(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}), flush=True)


if __name__ == "__main__":
    main()
