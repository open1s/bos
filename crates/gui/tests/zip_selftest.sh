#!/usr/bin/env bash
# Check the exported archive with an independent reader.
#
# The Rust tests describe the format through a reader written in the same
# change, which proves self-consistency and nothing more. This runs the real
# exporter and reads what it wrote with Python's zipfile, so two independent
# implementations have to agree about signatures, offsets, checksums and names.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WS="$(cd "$ROOT/../.." && pwd)"
KEEP="$(mktemp -d -t bos-export-guard)"
trap 'rm -rf "$KEEP"' EXIT

OUT="$(cd "$WS" && BOS_EXPORT_KEEP="$KEEP" cargo test -p bsh exporting_writes_where_it_says_it_wrote -- --nocapture 2>&1)"
PATH_TO_ZIP="$(printf '%s\n' "$OUT" | grep -oE 'BOS_EXPORT_PATH=.*' | head -1 | cut -d= -f2-)"
if [ -z "$PATH_TO_ZIP" ] || [ ! -f "$PATH_TO_ZIP" ]; then
  echo "[zip] failures: 1"
  echo "[zip] the exporter produced no archive; output tail:"
  printf '%s\n' "$OUT" | tail -12
  exit 1
fi

python3 - "$PATH_TO_ZIP" <<'PY'
import json, sys, zipfile

path = sys.argv[1]
checks, failures = [], []
def check(name, ok):
    checks.append((name, ok))
    if not ok:
        failures.append(name)

zf = zipfile.ZipFile(path)
check("the archive passes its own integrity test", zf.testzip() is None)
names = zf.namelist()
check("it holds exactly the three documented entries", names == ["README.txt", "chat.json", "chat.md"])
check("the comment field is empty", zf.comment == b"")
try:
    record = json.loads(zf.read("chat.json"))
    check("chat.json parses and carries messages", isinstance(record.get("messages"), list) and record["messages"])
    check("the folded-turn key survives", "archived" in record)
    check("the message text is in the record", record["messages"][0]["text"] == "hello")
except Exception as exc:
    check(f"chat.json parses ({exc})", False)
md = zf.read("chat.md").decode("utf-8")
check("chat.md names both roles and the ask", "## User" in md and "hello" in md)
check("README explains what the entries are", b"chat.json" in zf.read("README.txt"))
infos = zf.infolist()
check("entries are stored, not compressed", all(i.compress_type == zipfile.ZIP_STORED for i in infos))
check("every entry keeps its size and checksum", all(i.file_size == i.compress_size for i in infos))

for name, ok in checks:
    print(f"[zip]   {'ok  ' if ok else 'FAIL'} {name}")
print(f"[zip] checks={len(checks)} failures={len(failures)}")
sys.exit(1 if failures else 0)
PY
