#!/usr/bin/env python3
"""Exam the all-in-one BSH extension demo.

Runs the developer CLI (validate / test / mock / install / providers / doctor)
and drives the process protocol directly, then prints a pass/fail scoreboard.

    python3 scripts/exam.py
    BSH_BIN=/path/to/bsh python3 scripts/exam.py
    python3 scripts/exam.py --quick     # skip install and CLI-only checks
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

try:
    import tomllib
except ImportError:  # the extension runtime itself supports 3.8+; the exam needs 3.11+
    sys.exit("scripts/exam.py needs Python 3.11+ (tomllib)")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = []


def record(name, ok, detail=""):
    RESULTS.append((name, bool(ok), detail))
    print(("[PASS] " if ok else "[FAIL] ") + name + ((" · " + detail) if detail else ""))


def run(arguments, timeout=90):
    completed = subprocess.run(arguments, capture_output=True, text=True, timeout=timeout)
    return completed.returncode, completed.stdout, completed.stderr


def find_bsh(explicit):
    if explicit:
        return explicit
    if os.environ.get("BSH_BIN"):
        return os.environ["BSH_BIN"]
    found = shutil.which("bsh")
    if found:
        return found
    here = ROOT
    for _ in range(6):
        candidate = os.path.join(here, "target", "debug", "bsh")
        if os.path.isfile(candidate):
            return candidate
        here = os.path.dirname(here)
    return ""


def manifest():
    with open(os.path.join(ROOT, "extension.toml"), "rb") as handle:
        return tomllib.load(handle)


FILES = {
    "tools": "tools/{}.json",
    "commands": "commands/{}.md",
    "panels": "panels/{}.json",
    "prompts": "prompts/{}.md",
    "contexts": "contexts/{}.md",
    "skills": "skills/{}/SKILL.md",
    "languages": "languages/{}.json",
    "integrations": "integrations/{}.json",
    "themes": "themes/{}.json",
    "presets": "presets/{}.json",
    "model_providers": "providers/models/{}.json",
    "agent_providers": "providers/agents/{}.json",
    "workflows": "workflows/{}.json",
}


def check_manifest():
    document = manifest()
    contributes = document.get("contributes", {})
    missing = []
    for kind, pattern in FILES.items():
        for identifier in contributes.get(kind, []):
            path = os.path.join(ROOT, pattern.format(identifier))
            if not os.path.isfile(path):
                missing.append(os.path.relpath(path, ROOT))
    record("every declared contribution has its file", not missing, ", ".join(missing[:4]))
    schema = document.get("configuration", {}).get("schema")
    record("configuration schema is declared and present",
           bool(schema) and os.path.isfile(os.path.join(ROOT, schema)))
    events = contributes.get("events", [])
    record("event subscriptions are declared", len(events) >= 10, "%d topics" % len(events))
    record("provider descriptors are declared",
           bool(contributes.get("model_providers")) and bool(contributes.get("agent_providers")))


def check_json_files():
    bad = []
    for directory, _, names in os.walk(ROOT):
        for name in names:
            if not name.endswith(".json"):
                continue
            path = os.path.join(directory, name)
            try:
                with open(path, "r", encoding="utf-8") as handle:
                    json.load(handle)
            except Exception as error:
                bad.append("%s: %s" % (os.path.relpath(path, ROOT), error))
    record("every JSON contribution parses", not bad, "; ".join(bad[:3]))


def node_actions(node):
    found = []
    if isinstance(node, dict):
        if node.get("type") in ("button", "input", "select", "checkbox") and node.get("action"):
            found.append(node["action"])
        for item in node.get("items", []):
            if isinstance(item, dict) and item.get("action"):
                found.append(item["action"])
        for value in node.values():
            found.extend(node_actions(value))
    elif isinstance(node, list):
        for item in node:
            found.extend(node_actions(item))
    return found


def check_surfaces():
    document = manifest()
    contributes = document.get("contributes", {})
    tools = set(contributes.get("tools", []))
    commands = set(contributes.get("commands", []))
    declared = tools | commands
    unknown = []
    for kind in ("panels", "languages", "integrations"):
        for identifier in contributes.get(kind, []):
            path = os.path.join(ROOT, FILES[kind].format(identifier))
            with open(path, "r", encoding="utf-8") as handle:
                data = json.load(handle)
            if kind == "panels":
                for action in node_actions(data.get("view", {})):
                    if action not in declared:
                        unknown.append("%s: %s" % (identifier, action))
            else:
                action = data.get("tool") if kind == "languages" else data.get("action")
                if action not in declared:
                    unknown.append("%s: %s" % (identifier, action))
    record("panel, language and integration actions are declared", not unknown,
           ", ".join(unknown[:3]))
    bad_steps = []
    for identifier in contributes.get("workflows", []):
        path = os.path.join(ROOT, FILES["workflows"].format(identifier))
        with open(path, "r", encoding="utf-8") as handle:
            workflow = json.load(handle)
        for step in workflow.get("steps", []):
            if step.get("tool") not in tools:
                bad_steps.append("%s: %s" % (identifier, step.get("tool")))
    record("workflow steps call this package's tools", not bad_steps,
           ", ".join(bad_steps[:3]))


def check_protocol():
    process = subprocess.Popen(
        [sys.executable, "-B", "server.py"], cwd=ROOT,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    next_id = [0]

    def call(method, params, timeout=10):
        next_id[0] += 1
        identifier = next_id[0]
        process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": identifier,
                                        "method": method, "params": params}) + "\n")
        process.stdin.flush()
        deadline = time.time() + timeout
        while time.time() < deadline:
            line = process.stdout.readline()
            if not line:
                break
            message = json.loads(line)
            if message.get("id") == identifier:
                return message
        raise RuntimeError("no response to %s" % method)

    try:
        initialized = call("initialize", {"protocolVersion": "1.0", "token": "exam-token",
                                          "config": json.dumps({"greeting": "exam"})})
        record("process handshake echoes the host token",
               initialized.get("result", {}).get("token") == "exam-token")
        described = call("tools/call", {"name": "describe", "arguments": {}})
        result = described.get("result", {})
        record("describe reports the package and its surfaces",
               result.get("id") == "com.example.allinone" and len(result.get("surfaces", [])) >= 10)
        summed = call("tools/call", {"name": "sum", "arguments": {"a": 2, "b": 3}})
        record("typed arguments cross the bridge", summed.get("result", {}).get("sum") == 5)
        action = call("commands/execute", {"name": "refresh", "arguments": {}})
        document = action.get("result", {})
        record("a panel action returns a UI document",
               document.get("schema_version") == "1.0" and document.get("root", {}).get("type") == "column")
        process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "events/event",
                                        "params": {"event": {"event_type": "tool.completed",
                                                             "source": "exam", "timestamp": 1,
                                                             "payload": {"ok": True}}}}) + "\n")
        process.stdin.flush()
        logged = call("tools/call", {"name": "events", "arguments": {"limit": "5"}})
        record("events reach the process and are reported",
               logged.get("result", {}).get("count") == 1)
        failed = call("tools/call", {"name": "fail", "arguments": {"message": "exam failure"}})
        record("a tool failure is a JSON-RPC error",
               failed.get("error", {}).get("message") == "exam failure")
        call("shutdown", {})
    finally:
        process.kill()


def check_cli(binary):
    if not binary:
        record("BSH binary found", False, "set BSH_BIN or pass --binary")
        return
    code, out, err = run([binary, "ext", "validate", ROOT])
    record("bsh ext validate", code == 0 and "valid" in out, out.strip().splitlines()[-1] if out else err)
    code, out, err = run([binary, "ext", "test", ROOT])
    counts = {}
    for line in out.splitlines():
        if ":" in line:
            key, _, value = line.partition(":")
            counts[key.strip()] = value.strip()
    record("bsh ext test counts the contributions",
           code == 0 and counts.get("tools") == "7" and counts.get("commands") == "2"
           and counts.get("panels") == "1" and counts.get("workflows") == "1",
           "tools=%s commands=%s panels=%s workflows=%s" % (counts.get("tools"), counts.get("commands"),
                                                           counts.get("panels"), counts.get("workflows")))

    echoes = {
        "describe": ("{}", lambda value: value.get("id") == "com.example.allinone"),
        "echo": ('{"text":"exam"}', lambda value: value.get("text") == "exam"),
        "sum": ('{"a":7,"b":5}', lambda value: value.get("sum") == 12),
        "progress": ('{"steps":2}', lambda value: value.get("steps") == 2),
        "config": ("{}", lambda value: value.get("configuration", {}).get("greeting")),
        "events": ('{"limit":3}', lambda value: value.get("count") == 0),
    }
    for tool, (arguments, predicate) in echoes.items():
        code, out, err = run([binary, "ext", "mock", ROOT, tool, arguments])
        try:
            value = json.loads(out)
        except Exception:
            value = {}
        ok = code == 0 and predicate(value)
        record("bsh ext mock %s" % tool, ok,
               "" if ok else (err.strip() or out.strip())[:160])
    code, out, err = run([binary, "ext", "mock", ROOT, "fail", '{"message":"on purpose"}'])
    record("bsh ext mock fail returns a refusal",
           code != 0 and "on purpose" in (out + err))
    code, out, err = run([binary, "ext", "mock", ROOT, "progress", '{"steps":2}'])
    record("process progress reaches the host", "step 1 of 2" in (out + err))


def check_install(binary):
    if not binary:
        return
    scratch = tempfile.mkdtemp(prefix="bsh-allinone-exam-")
    try:
        identifier = manifest()["extension"]["id"]
        run([binary, "ext", "--root", scratch, "grant", identifier, "process.execute",
             "--scope", "program:python3", "--note", "exam"])
        code, out, err = run([binary, "ext", "--root", scratch, "install", ROOT])
        record("bsh ext install", code == 0 and identifier in out,
               "" if code == 0 else (err or out).strip()[-160:])
        code, out, err = run([binary, "ext", "--root", scratch, "list"])
        record("installed package is active", code == 0 and identifier in out and "active" in out)
        code, out, err = run([binary, "ext", "--root", scratch, "providers"])
        try:
            providers = json.loads(out)
        except Exception:
            providers = []
        record("model and agent providers are listed",
               any(p.get("kind") == "model" and p.get("id") == "demo" for p in providers)
               and any(p.get("kind") == "agent" and p.get("id") == "helper" for p in providers))
        code, out, err = run([binary, "ext", "--root", scratch, "languages"])
        record("language descriptor is listed", code == 0 and "allinone" in out)
        code, out, err = run([binary, "ext", "--root", scratch, "integrations"])
        record("integration descriptors are listed",
               code == 0 and "github" in out and "command" in out)
        code, out, err = run([binary, "ext", "--root", scratch, "doctor"])
        record("bsh ext doctor is clean", code == 0 and "0 error" in out.lower(),
               "0 errors" if "0 error" in out.lower() else out.strip()[-120:])
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser(description="Exam the all-in-one BSH extension demo")
    parser.add_argument("--binary", default="", help="path to the bsh binary")
    parser.add_argument("--quick", action="store_true", help="skip CLI install checks")
    arguments = parser.parse_args()
    binary = find_bsh(arguments.binary)

    print("BSH Extension Framework — All-in-One Exam")
    print("package:", ROOT)
    print("binary: ", binary or "(not found)")
    print()
    check_manifest()
    check_json_files()
    check_surfaces()
    check_protocol()
    check_cli(binary)
    if not arguments.quick:
        check_install(binary)

    passed = sum(1 for _, ok, _ in RESULTS if ok)
    print()
    print("Score: %d/%d" % (passed, len(RESULTS)))
    return 0 if passed == len(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
