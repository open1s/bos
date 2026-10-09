"""Read one headless-harness result block and report it.

Shared by the guards so that every runner treats a missing or unparseable
result the same way. Usage: read_result.py <page-tag> <dumped-dom-file>
Exits non-zero when the result is missing or any check failed.
"""
import html as H
import json
import re
import sys

page, path = sys.argv[1], sys.argv[2]
raw = open(path, encoding="utf-8", errors="replace").read()
# Any element carrying id="result" — a load-error report can be written into a
# plain element, and demanding <pre> made this runner report such a page as
# "result block missing", i.e. as a hang, which cost six rounds of chasing it.
m = re.search(r'<[a-zA-Z]+[^>]*\bid="result"[^>]*>(.*?)</[a-zA-Z]+>', raw, re.S)
if not m:
    print(f'[{page}] no result element (id="result") — page did not finish')
    raise SystemExit(1)
d = json.loads(H.unescape(m.group(1)))
for k, v in d.items():
    if k not in ("checks", "failures", "total", "page"):
        print(f"[{page}] {k}: {v}")
checks = d.get("checks")
rows = (
    [(k, str(v).startswith("PASS")) for k, v in checks.items()]
    if isinstance(checks, dict)
    else [(c["name"], bool(c["ok"])) for c in (checks or [])]
)
for name, ok in rows:
    print(f"[{page}] " + ("  ok  " if ok else " FAIL ") + name)
failures = int(d.get("failures") or 0)
print(f"[{page}] failures: {failures} / {len(rows)}\n")
raise SystemExit(1 if failures else 0)
