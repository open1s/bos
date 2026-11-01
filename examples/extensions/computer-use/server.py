#!/usr/bin/env python3
"""A real macOS computer-use process extension for BSH.

Transport: newline-delimited JSON-RPC 2.0, exactly like every BSH process
runtime. It uses CoreGraphics through ctypes, so it needs no Python packages.

Permissions: macOS Screen Recording for screenshots and Accessibility for
pointer/keyboard events. Without them the OS may silently ignore input; the
README explains the checks and the safe test sequence.
"""

from __future__ import annotations

import ctypes
import ctypes.util
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

POINT = None
CG = None
CF = None


def load_frameworks():
    global POINT, CG, CF
    if CG is not None:
        return True
    if sys.platform != "darwin":
        return False
    CG = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
    CF = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")

    class CGPoint(ctypes.Structure):
        _fields_ = [("x", ctypes.c_double), ("y", ctypes.c_double)]

    class CGSize(ctypes.Structure):
        _fields_ = [("width", ctypes.c_double), ("height", ctypes.c_double)]

    class CGRect(ctypes.Structure):
        _fields_ = [("origin", CGPoint), ("size", CGSize)]

    POINT = CGPoint
    CG.CGEventCreate.argtypes = [ctypes.c_void_p]
    CG.CGEventCreate.restype = ctypes.c_void_p
    CG.CGEventCreateMouseEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint32, CGPoint, ctypes.c_uint32]
    CG.CGEventCreateMouseEvent.restype = ctypes.c_void_p
    CG.CGEventCreateKeyboardEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint16, ctypes.c_bool]
    CG.CGEventCreateKeyboardEvent.restype = ctypes.c_void_p
    CG.CGEventCreateScrollWheelEvent2.argtypes = [
        ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32,
        ctypes.c_int32, ctypes.c_int32, ctypes.c_int32,
    ]
    CG.CGEventCreateScrollWheelEvent2.restype = ctypes.c_void_p
    CG.CGEventSetFlags.argtypes = [ctypes.c_void_p, ctypes.c_uint64]
    CG.CGEventSetIntegerValueField.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int64]
    CG.CGEventKeyboardSetUnicodeString.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_uint16)]
    CG.CGEventPost.argtypes = [ctypes.c_uint32, ctypes.c_void_p]
    CG.CGEventGetLocation.argtypes = [ctypes.c_void_p]
    CG.CGEventGetLocation.restype = CGPoint
    CG.CGWarpMouseCursorPosition.argtypes = [CGPoint]
    CG.CGMainDisplayID.restype = ctypes.c_uint32
    CG.CGDisplayPixelsWide.argtypes = [ctypes.c_uint32]
    CG.CGDisplayPixelsWide.restype = ctypes.c_size_t
    CG.CGDisplayPixelsHigh.argtypes = [ctypes.c_uint32]
    CG.CGDisplayPixelsHigh.restype = ctypes.c_size_t
    CG.CGDisplayBounds.argtypes = [ctypes.c_uint32]
    CG.CGDisplayBounds.restype = CGRect
    CF.CFRelease.argtypes = [ctypes.c_void_p]
    CF.CFRelease.restype = None
    return True


HID = 0
LEFT, RIGHT, MIDDLE = 0, 1, 2
DOWN = {LEFT: 1, RIGHT: 3, MIDDLE: 25}
UP = {LEFT: 2, RIGHT: 4, MIDDLE: 26}
DRAG = {LEFT: 6, RIGHT: 7}
MOVE = 5

FLAGS = {
    "cmd": 1 << 20,
    "shift": 1 << 17,
    "alt": 1 << 19,
    "ctrl": 1 << 18,
    "fn": 1 << 23,
}

KEYS = {
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7,
    "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15,
    "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22,
    "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29,
    "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37,
    "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44,
    "n": 45, "m": 46, ".": 47, "`": 50,
    "return": 36, "enter": 36, "tab": 48, "space": 49, "delete": 51,
    "backspace": 51, "escape": 53, "esc": 53,
    "left": 123, "right": 124, "down": 125, "up": 126,
    "home": 115, "end": 119, "pageup": 116, "pagedown": 121,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97,
    "f7": 98, "f8": 100, "f9": 101, "f10": 109, "f11": 103, "f12": 111,
    "cmd": 55, "shift": 56, "alt": 58, "option": 58, "ctrl": 59, "control": 59,
}

EVENTS: list[dict] = []


def jsonrpc(rid, result=None, error=None):
    message = {"jsonrpc": "2.0", "id": rid}
    if error is not None:
        message["error"] = {"code": -32000, "message": str(error)}
    else:
        message["result"] = result
    sys.stdout.write(json.dumps(message, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def need_macos():
    if not load_frameworks():
        raise RuntimeError("Computer Use currently supports macOS only")


def main_display():
    need_macos()
    display = CG.CGMainDisplayID()
    bounds = CG.CGDisplayBounds(display)
    width = int(bounds.size.width)
    height = int(bounds.size.height)
    return {
        "display": display,
        "width": width,
        "height": height,
        "origin_x": bounds.origin.x,
        "origin_y": bounds.origin.y,
        "pixel_width": int(CG.CGDisplayPixelsWide(display)),
        "pixel_height": int(CG.CGDisplayPixelsHigh(display)),
    }


def png_size(path: str):
    with open(path, "rb") as handle:
        head = handle.read(24)
    if len(head) < 24 or head[:8] != b"\x89PNG\r\n\x1a\n":
        return 0, 0
    return int.from_bytes(head[16:20], "big"), int.from_bytes(head[20:24], "big")


def screenshot(path: str = ""):
    need_macos()
    if not path:
        path = str(Path(tempfile.gettempdir()) / f"bsh-computer-{int(time.time() * 1000)}.png")
    path = os.path.abspath(os.path.expanduser(path))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    proc = subprocess.run(
        ["/usr/sbin/screencapture", "-x", "-t", "png", path],
        capture_output=True, text=True, timeout=15,
    )
    if proc.returncode != 0:
        raise RuntimeError((proc.stderr or proc.stdout or "screencapture failed").strip())
    if not os.path.exists(path) or os.path.getsize(path) == 0:
        raise RuntimeError(
            "screencapture produced no image; grant Screen Recording to the BSH app "
            "in System Settings > Privacy & Security > Screen Recording"
        )
    width, height = png_size(path)
    return {
        "ok": True,
        "path": path,
        "pixel_width": width,
        "pixel_height": height,
        "bytes": os.path.getsize(path),
        "note": "Screenshot pixels are not mouse points; use screen_size or divide by the Retina scale.",
    }


def cursor_position():
    need_macos()
    event = CG.CGEventCreate(None)
    if not event:
        raise RuntimeError("CGEventCreate failed")
    point = CG.CGEventGetLocation(event)
    CF.CFRelease(event)
    return {"ok": True, "x": point.x, "y": point.y}


def mouse_event(kind, x, y, button=LEFT, click_state=0):
    event = CG.CGEventCreateMouseEvent(None, kind, POINT(float(x), float(y)), button)
    if not event:
        raise RuntimeError("CGEventCreateMouseEvent failed")
    if click_state:
        CG.CGEventSetIntegerValueField(event, 1, click_state)
    CG.CGEventPost(HID, event)
    CF.CFRelease(event)


def mouse_move(x, y):
    need_macos()
    CG.CGWarpMouseCursorPosition(POINT(float(x), float(y)))
    mouse_event(MOVE, x, y)
    return {"ok": True, "x": x, "y": y}


def mouse_click(x, y, button="left", clicks=1):
    need_macos()
    number = {"left": LEFT, "right": RIGHT, "middle": MIDDLE}.get(str(button), LEFT)
    count = max(1, min(3, int(clicks)))
    CG.CGWarpMouseCursorPosition(POINT(float(x), float(y)))
    for state in range(1, count + 1):
        mouse_event(DOWN[number], x, y, number, state)
        mouse_event(UP[number], x, y, number, state)
        if state != count:
            time.sleep(0.06)
    return {"ok": True, "x": x, "y": y, "button": button, "clicks": count}


def mouse_drag(from_x, from_y, to_x, to_y, button="left", duration_ms=300):
    need_macos()
    number = RIGHT if str(button) == "right" else LEFT
    duration = max(0.05, min(2.0, float(duration_ms) / 1000.0))
    CG.CGWarpMouseCursorPosition(POINT(float(from_x), float(from_y)))
    mouse_event(DOWN[number], from_x, from_y, number)
    steps = 12
    for index in range(1, steps + 1):
        ratio = index / steps
        x = float(from_x) + (float(to_x) - float(from_x)) * ratio
        y = float(from_y) + (float(to_y) - float(from_y)) * ratio
        mouse_event(DRAG[number], x, y, number)
        time.sleep(duration / steps)
    mouse_event(UP[number], to_x, to_y, number)
    return {"ok": True, "from": [from_x, from_y], "to": [to_x, to_y]}


def scroll(delta_y, delta_x=0, x=None, y=None):
    need_macos()
    if x is not None and y is not None:
        CG.CGWarpMouseCursorPosition(POINT(float(x), float(y)))
    event = CG.CGEventCreateScrollWheelEvent2(None, 0, 2, int(delta_y), int(delta_x), 0)
    if not event:
        raise RuntimeError("CGEventCreateScrollWheelEvent2 failed")
    CG.CGEventPost(HID, event)
    CF.CFRelease(event)
    return {"ok": True, "delta_x": int(delta_x), "delta_y": int(delta_y)}


def type_text(text):
    need_macos()
    encoded = str(text).encode("utf-16-le")
    units = [int.from_bytes(encoded[i:i + 2], "little") for i in range(0, len(encoded), 2)]
    for start in range(0, len(units), 20):
        chunk = units[start:start + 20]
        event = CG.CGEventCreateKeyboardEvent(None, 0, True)
        buffer = (ctypes.c_uint16 * len(chunk))(*chunk)
        CG.CGEventKeyboardSetUnicodeString(event, len(chunk), buffer)
        CG.CGEventPost(HID, event)
        CF.CFRelease(event)
        up = CG.CGEventCreateKeyboardEvent(None, 0, False)
        CG.CGEventPost(HID, up)
        CF.CFRelease(up)
    return {"ok": True, "characters": len(str(text))}


def key_press(key, modifiers=None):
    need_macos()
    parts = str(key).lower().split("+")
    names = [part.strip() for part in parts if part.strip()]
    mods = [str(item).lower() for item in (modifiers or [])]
    mods.extend(part for part in names[:-1] if part in FLAGS)
    key_name = names[-1] if names else ""
    code = KEYS.get(key_name)
    if code is None:
        raise RuntimeError(f"unknown key: {key_name}")
    flags = 0
    for mod in mods:
        flags |= FLAGS.get(mod, 0)
    down = CG.CGEventCreateKeyboardEvent(None, code, True)
    CG.CGEventSetFlags(down, flags)
    CG.CGEventPost(HID, down)
    CF.CFRelease(down)
    up = CG.CGEventCreateKeyboardEvent(None, code, False)
    CG.CGEventSetFlags(up, flags)
    CG.CGEventPost(HID, up)
    CF.CFRelease(up)
    return {"ok": True, "key": key_name, "modifiers": sorted(set(mods))}


def call_tool(name, args):
    args = args or {}
    if name == "screen_size":
        info = main_display()
        info["ok"] = True
        return info
    if name == "screenshot":
        return screenshot(str(args.get("path", "")))
    if name == "cursor_position":
        return cursor_position()
    if name == "mouse_move":
        return mouse_move(args["x"], args["y"])
    if name == "mouse_click":
        return mouse_click(args["x"], args["y"], args.get("button", "left"), args.get("clicks", 1))
    if name == "mouse_drag":
        return mouse_drag(
            args["from_x"], args["from_y"], args["to_x"], args["to_y"],
            args.get("button", "left"), args.get("duration_ms", 300),
        )
    if name == "scroll":
        return scroll(args["delta_y"], args.get("delta_x", 0), args.get("x"), args.get("y"))
    if name == "type_text":
        return type_text(args["text"])
    if name == "key_press":
        return key_press(args["key"], args.get("modifiers"))
    raise RuntimeError(f"unknown tool: {name}")


def panel_document():
    screen = main_display()
    cursor = cursor_position()
    last = EVENTS[-1]["topic"] if EVENTS else "none"
    return {
        "schema_version": "1.0",
        "revision": 2,
        "root": {
            "type": "column",
            "children": [
                {"type": "heading", "text": "Computer Use", "level": 2},
                {"type": "text", "text": f"Display: {screen['width']} x {screen['height']} points ({screen['pixel_width']} x {screen['pixel_height']} px)"},
                {"type": "text", "text": f"Pointer: {cursor['x']:.0f}, {cursor['y']:.0f}"},
                {"type": "text", "text": f"Events: {len(EVENTS)} (last: {last})"},
                {"type": "button", "label": "Screenshot", "action": "screenshot", "style": "primary"},
                {"type": "button", "label": "Refresh", "action": "refresh"},
            ],
        },
    }


def command(name, args):
    if name == "refresh":
        return panel_document()
    raise RuntimeError(f"unknown command: {name}")


def run():
    load_frameworks()
    token = os.environ.get("BSH_IPC_TOKEN", "")
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError as error:
            jsonrpc(None, error=f"invalid JSON: {error}")
            continue
        rid = request.get("id")
        method = request.get("method")
        params = request.get("params") or {}
        if method == "initialize":
            provided = params.get("token", "")
            if token and provided and provided != token:
                jsonrpc(rid, error="IPC token mismatch")
                return
            jsonrpc(rid, result={"protocolVersion": "1.0", "token": provided or token})
        elif method == "tools/call":
            try:
                jsonrpc(rid, result=call_tool(params.get("name", ""), params.get("arguments") or {}))
            except Exception as error:  # noqa: BLE001 - the protocol returns an error value
                jsonrpc(rid, result={"ok": False, "error": str(error)})
        elif method == "commands/execute":
            try:
                jsonrpc(rid, result=command(params.get("name", ""), params.get("arguments") or {}))
            except Exception as error:  # noqa: BLE001
                jsonrpc(rid, result={"ok": False, "error": str(error)})
        elif method == "events/event":
            event = params.get("event") or {}
            EVENTS.append({"topic": event.get("event_type", "?"), "payload": event.get("payload")})
            del EVENTS[:-20]
        elif method == "shutdown":
            jsonrpc(rid, result={})
            return
        else:
            jsonrpc(rid, error=f"unknown method: {method}")


if __name__ == "__main__":
    run()
