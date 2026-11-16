#!/usr/bin/env python3
"""
Config Demo — Discovery and loading

Demonstrates:
1. Config discover (auto-find ~/.bos/conf/config.toml)
2. Adding files and inline overrides
3. Loading and reloading config
"""

from nbos.config import Config


def demo_discover():
    print("═" * 60)
    print("  Demo 1 — Config discover")
    print("═" * 60)

    cfg = Config()
    cfg.discover()
    data = cfg.load_sync()

    if data:
        print(f"  📄 Loaded config with keys: {list(data.keys())}")
        if "global_model" in data:
            print(f"  🤖 Model: {data['global_model'].get('model', 'N/A')}")
        if "bus" in data:
            print(f"  🚌 Bus mode: {data['bus'].get('mode', 'N/A')}")
    else:
        print("  ℹ️  No config file found (create ~/.bos/conf/config.toml)")
    print()


def demo_inline_override():
    print("═" * 60)
    print("  Demo 2 — Inline config override")
    print("═" * 60)

    cfg = Config()
    cfg.add_inline({
        "agent": {
            "name": "demo-agent",
            "model": "gpt-4",
            "temperature": 0.5,
        },
        "features": ["chat", "tools"],
    })
    data = cfg.load_sync()
    print(f"  📄 Agent name: {data['agent']['name']}")
    print(f"  📄 Agent model: {data['agent']['model']}")
    print(f"  📄 Features: {data['features']}")
    print()


def demo_reload():
    print("═" * 60)
    print(" Demo 3 — Config reload")
    print("═" * 60)

    cfg = Config()
    cfg.add_inline({"version": 1})
    initial = cfg.load_sync()
    print(f" 📄 Initial: {initial}")

    cfg.reset()
    cfg.add_inline({"version": 2, "new_key": "added"})
    reloaded = cfg.load_sync()
    print(f" 📄 After reload: {reloaded}")
    print()


def main():
    print("\n" + "⚙️" * 30)
    print("  BrainOS — Config Demo")
    print("⚙️" * 30 + "\n")

    demo_discover()
    demo_inline_override()
    demo_reload()

    print("═" * 60)
    print("  ✅ All Config demos completed!")
    print("═" * 60 + "\n")


if __name__ == "__main__":
    main()
