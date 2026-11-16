"""Language and integration starter.

Run a diagnostic with:

    bsh ext language-check com.example.language-python python sample.py

Dispatch the integration with:

    bsh ext integrate com.example.language-python github '{"query":"bsh"}'
"""

import ast
import json
import sys
import urllib.parse


def diagnose(arguments):
    text = arguments.get("text", "")
    try:
        ast.parse(text)
    except SyntaxError as error:
        return {
            "ok": False,
            "diagnostics": [{
                "message": error.msg,
                "line": error.lineno or 0,
                "column": error.offset or 0,
                "severity": "error",
            }],
        }
    return {"ok": True, "diagnostics": []}


def github_search(arguments):
    query = arguments.get("query", "")
    return {
        "url": "https://github.com/search?q=" + urllib.parse.quote(query),
        "query": query,
    }


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
        request_id = request.get("id")
        if method == "initialize":
            result = {
                "protocolVersion": "1.0",
                "token": params.get("token"),
                "name": "language-python",
            }
        elif method == "tools/call":
            name = params.get("name")
            arguments = params.get("arguments", {})
            if name == "diagnose":
                result = diagnose(arguments)
            elif name == "github-search":
                result = github_search(arguments)
            else:
                result = {"error": f"unknown tool {name}"}
        elif method == "commands/execute":
            result = {"id": params.get("name"), "arguments": params.get("arguments", {})}
        elif method == "shutdown":
            result = {}
        else:
            result = {}
        print(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}), flush=True)


if __name__ == "__main__":
    main()
