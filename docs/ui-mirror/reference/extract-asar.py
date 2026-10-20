#!/usr/bin/env python3
"""List, read or extract entries from a shipped Electron asar.

The DeepSeek Harness app ships its renderer as an asar. Its layout is the
standard one: a 4-byte u32 at offset 0, a u32 pickle size at 4, a u32 JSON
size at 8, then the header JSON. File data starts at 8 + pickle size and each
leaf's offset is relative to that base. (The common 16 + json size reading is
two bytes off and silently prefixes every file with the previous file tail.)

Usage:
    python3 extract-asar.py --list
    python3 extract-asar.py --read <entry-path>
    python3 extract-asar.py [--asar PATH] [--out DIR] [entry-path ...]
"""

import argparse
import json
import os
import struct
import sys

DEFAULT_ASAR = "/Applications/DeepSeek Harness.app/Contents/Resources/app.asar"


def load(path):
    data = open(path, "rb").read()
    _, pickle, _, json_size = struct.unpack("<IIII", data[:16])
    base = 8 + pickle
    header = json.loads(data[16 : 16 + json_size])
    entries = []

    def walk(node, prefix):
        for name, value in node.get("files", {}).items():
            path = (prefix + "/" + name) if prefix else name
            if "files" in value:
                walk(value, path)
            elif value.get("offset") is not None:
                entries.append((int(value["offset"]), int(value.get("size", 0)), path))

    walk(header, "")
    return data, base, entries


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--asar", default=DEFAULT_ASAR)
    parser.add_argument("--out", default=".")
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--read")
    parser.add_argument("selectors", nargs="*")
    args = parser.parse_args()

    data, base, entries = load(args.asar)

    if args.list:
        for _, size, path in entries:
            print(size, path)
        return

    if args.read:
        for offset, size, path in entries:
            if path == args.read:
                sys.stdout.write(data[base + offset : base + offset + size].decode("utf-8", "replace"))
                return
        raise SystemExit("no such entry: " + args.read)

    selectors = args.selectors or ["dsh-"]
    os.makedirs(args.out, exist_ok=True)
    written = 0
    for offset, size, path in entries:
        if not any(selector in path for selector in selectors):
            continue
        name = path.replace("dsh/node_modules/@deepseek-ai/", "").replace("/", "__")
        with open(os.path.join(args.out, name), "wb") as handle:
            handle.write(data[base + offset : base + offset + size])
        written += 1
    print("extracted " + str(written) + " files to " + args.out)


if __name__ == "__main__":
    main()