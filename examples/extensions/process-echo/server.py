"""A minimal BSH process extension.

The host writes one JSON object per line and reads one JSON object per line.
This server answers initialize, tools/call and shutdown; every other method
returns an empty result, which the host treats as a protocol-level no-op.
"""

import json
import sys


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
        if method == "initialize":
            result = {"protocolVersion": "1.0"}
        elif method == "tools/call":
            result = request.get("params", {}).get("arguments", {})
        elif method == "shutdown":
            result = {}
        else:
            result = {}
        response = {"jsonrpc": "2.0", "id": request.get("id"), "result": result}
        print(json.dumps(response), flush=True)


if __name__ == "__main__":
    main()
