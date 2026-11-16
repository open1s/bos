"""First-party workspace notes extension."""

import json
import os
import sys


def store_path():
    return os.path.join(os.getcwd(), ".bsh-notes.json")


def load():
    try:
        with open(store_path(), encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, json.JSONDecodeError):
        return []


def save(arguments):
    notes = load()
    notes.append(str(arguments.get("text", "")))
    with open(store_path(), "w", encoding="utf-8") as handle:
        json.dump(notes, handle)
    return {"saved": len(notes)}


def list_notes(_arguments):
    return {"notes": load()}


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
        params = request.get("params", {})
        if method == "initialize":
            result = {
                "protocolVersion": "1.0",
                "token": params.get("token"),
            }
        elif method == "tools/call":
            name = params.get("name")
            arguments = params.get("arguments", {})
            result = save(arguments) if name == "save" else list_notes(arguments)
        elif method == "shutdown":
            result = {}
        else:
            result = {}
        print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)


if __name__ == "__main__":
    main()
