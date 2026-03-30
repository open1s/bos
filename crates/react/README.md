ReAct crate (Rust) - Reasoning + Acting loop scaffold

- Plan A: QA, tests, release readiness
- Plan B: Production-ready scaffolding (robust prompts, persistent memory, multi-tool integration)

How to run tests
- cargo test -p react --workspace
- You can run the full workspace as a QA check: cargo test --workspace

Notes
- This crate provides a minimal yet extensible ReAct-style engine with a pluggable tool registry.
- Memory supports persistence to disk via save_to_file/load_from_file for cross-session memory.
- Times out LLM calls to guard against long delays; timeout duration can be tuned in engine.rs.
