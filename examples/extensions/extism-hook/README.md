# Extism Hook Observer

A runnable Extism extension that consumes BSH observe hooks.

It declares:

```toml
[capabilities]
required = ["hooks.observe", "extension.storage", "agent.tools.register", "ui.panels"]

[contributes]
tools = ["stats"]
panels = ["inspector"]
hooks = ["model.request", "model.response", "http.exchange"]
```

The host calls `bsh_hook` with `{"point": "...", "batch": [...]}`; the guest
keeps one counter per point in its storage namespace, exposes them through the
`stats` tool and the `inspector` panel, and returns `{"action":"continue"}`.

## Build the plugin

```text
cd guest
cargo build --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/bsh_extism_hook.wasm ../plugin.wasm
```

The committed `plugin.wasm` was built this way.

## Try it

```text
bsh ext install examples/extensions/extism-hook
bsh ext grant com.example.extism-hook hooks.observe --scope any
bsh ext grant com.example.extism-hook extension.storage --scope any
bsh ext grant com.example.extism-hook agent.tools.register --scope any
bsh ext grant com.example.extism-hook ui.panels --scope any
```

Each agent turn then delivers `model.request` / `model.response`; an extension
`fetch` produces `http.exchange`. Open the **Observe hooks** panel (or call the
`stats` tool) to read the counters.

`bsh ext mock` does not run an agent, so it shows zero counters: it proves the
runtime contract, not hook delivery.
