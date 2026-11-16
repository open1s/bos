# react — ReAct Reasoning + Acting Engine

> Core reasoning engine for the BrainOS agent framework. Implements the ReAct (Reason + Act) paradigm with multimodal content support, persistent memory, and pluggable LLM vendors.

## Features

- **ReAct Loop** — Iterative reasoning + acting with tool integration
- **Multimodal Content** — Text, images, and audio via `ContentPart::Text` / `ContentPart::Binary`
- **Multiple LLM Vendors** — OpenAI, NVIDIA NIM, OpenRouter
- **Streaming** — Token-level streaming for real-time responses
- **Memory** — Persistent session memory with save/load via `save_to_file`/`load_from_file`
- **Tool Registry** — Pluggable tool registry for LLM function calling
- **Configurable Timeout** — LLM call timeout with tunable duration

## Usage

```rust
use react::ReActEngine;

let engine = ReActEngine::new(config);
let response = engine.run("Hello").await?;
```

## Running Tests

```bash
# Test the react crate
cargo test -p react

# Full workspace tests
cargo test --workspace
```
