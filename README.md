# BrainOS (BOS)

A modular Rust-based operating system and runtime framework for building intelligent AI-powered applications with support for multi-agent coordination, event streaming, and extensible tool systems.

## 📦 Project Overview

BrainOS is a comprehensive framework designed to enable seamless integration of AI agents, messaging systems, configuration management, and tool execution. It provides production-ready components for building scalable, distributed AI systems.

### Key Features

- **🤖 Agent Framework**: Multi-agent coordination with LLM integration and skill management
- **🚌 Event Bus**: High-performance pub/sub messaging system with zenoh support
- **⚙️ Configuration Management**: Flexible config loading from TOML, YAML, and environment variables
- **🧠 ReAct Engine**: Reasoning + Acting loop scaffold for AI agent workflows
- **📊 Logging & Telemetry**: Integrated tracing and observability
- **🛠️ Extensible Tools**: Circuit breaker, HTTP client, search, calculator, and more
- **🔄 Memory Persistence**: Cross-session memory support for agents

---

## 🏗️ Project Structure

```
bos/
├── crates/
│   ├── agent/          # AI agent framework with LLM, skills, and tool management
│   ├── bus/            # Event streaming and pub/sub messaging
│   ├── config/         # Configuration loading and management
│   ├── logging/        # Logging and instrumentation
│   └── react/          # ReAct (Reasoning + Acting) AI engine
├── examples/           # Example applications and use cases
├── tools/              # Build and utility scripts
├── Cargo.toml          # Workspace configuration
├── CHANGELOG.md        # Version history and releases
└── CONTRIBUTING.md     # Contribution guidelines
```

### 📚 Crates

#### `agent` - AI Agent Framework
Core framework for building intelligent agents with LLM integration, dynamic skills, and plugin-based tool support.

**Key modules:**
- `llm/` - Language Model interface and integration
- `skills/` - Skill management and composition
- `tools/` - Tool registry, circuit breaker, and execution
- `mcp/` - Model Context Protocol support
- `session/` - Agent session management
- `error.rs` - Error types and handling

**Run tests:**
```bash
cargo test -p agent --lib
```

#### `bus` - Event Streaming & Messaging
High-performance pub/sub event bus with Zenoh integration for distributed messaging.

**Key modules:**
- `publisher.rs` - Event publishing interface
- `subscriber.rs` - Event subscription and consumption
- `callable.rs` - Callable event handlers
- `query.rs` - Query/response patterns
- `session.rs` - Session management

**Run tests:**
```bash
cargo test -p bus --lib
```

#### `config` - Configuration Management
Flexible configuration loading supporting TOML, YAML, environment variables, and glob patterns.

**Key modules:**
- `loader.rs` - Config file loading logic
- `types.rs` - Configuration types and schemas
- `error.rs` - Configuration errors

**Run tests:**
```bash
cargo test -p config --lib
```

#### `logging` - Logging & Instrumentation
Centralized logging setup with tracing integration for observability.

**Run tests:**
```bash
cargo test -p logging --lib
```

#### `react` - ReAct AI Engine
Reasoning + Acting loop scaffold providing a minimal yet extensible AI agent engine with tool support, timeouts, and memory persistence.

**Key features:**
- Pluggable tool registry
- LLM call timeouts
- Memory persistence (save/load to disk)
- Multi-tool integration
- Resilience and observability

**Run tests:**
```bash
cargo test -p react --lib
```

---

## 🚀 Getting Started

### Prerequisites

- **Rust** 1.70+ (Edition 2021)
- **Cargo**
- **Tokio** runtime (async support)

### Installation

Clone the repository:

```bash
git clone https://github.com/your-org/bos.git
cd bos
```

### Building

Build the entire workspace:

```bash
cargo build --release
```

Build a specific crate:

```bash
cargo build -p react --release
```

### Running Tests

Run all tests:

```bash
cargo test --all
```

Run tests for a specific crate:

```bash
cargo test -p react --all
cargo test -p agent --all
```

Run a specific test with output:

```bash
cargo test -p react -- --nocapture
```

### Running Examples

Navigate to the `examples/` directory and run:

```bash
cargo run --example <example-name>
```

---

## 📖 Core Concepts

### Agent System

Agents are autonomous entities that can:
- Understand LLM outputs and reasoning
- Execute tools and skills
- Manage sessions and state
- Integrate with external systems

### Event Bus

The bus provides:
- Topic-based pub/sub messaging
- Query/response patterns
- Session-scoped communication
- Support for distributed systems via Zenoh

### Tool Execution

Tools are composable units that:
- Support circuit breakers for resilience
- Integrate with LLM action outputs
- Provide observability and telemetry
- Can be chained and composed

### Memory Persistence

- In-memory storage for fast access
- File-based persistence for durability
- Cross-session state management

---

## 🛠️ Development Workflow

### Code Style

- Follows Rust edition 2021 best practices
- Uses clippy for linting
- Enforces formatting with rustfmt

### Benchmarking

The project includes criterion-based benchmarks:

```bash
cargo bench --all
```

### Profiling

With flamegraph support:

```bash
cargo flamegraph --bin <bin-name>
```

### Running QA Checks

Full workspace QA:

```bash
cargo test --workspace
```

Integration tests:

```bash
cargo test --test '*'
```

---

## 📋 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for:
- Code of conduct
- Development process
- Coding standards
- Commit conventions
- Testing requirements

### Key Guidelines

1. **Fork & Branch**: Create a feature branch
2. **Code Quality**: Run `cargo clippy` and `cargo fmt`
3. **Tests**: Add tests for new features
4. **Documentation**: Document public APIs
5. **Commit Messages**: Follow conventional commits

---

## 📝 Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history, current status, and upcoming features.

### Current Status

- **Plan A**: ReAct crate QA, warnings cleanup, release notes
- **Plan B**: Production-ready scaffolding (robust prompts, memory persistence, multi-tool integration)

---

## 🔧 Dependencies

### Key Workspace Dependencies

- **`tokio`** - Async runtime
- **`serde/serde_json`** - Serialization
- **`zenoh`** - Distributed pub/sub
- **`tracing`** - Structured logging
- **`reqwest`** - HTTP client
- **`anyhow/thiserror`** - Error handling
- **`async-trait`** - Async trait support

See `Cargo.toml` for complete dependency list.

---

## 📄 License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE) or http://opensource.org/licenses/MIT)

at your option.

---

## 👥 Authors

- **BrickOS Team**

---

## 🤝 Support

For issues, questions, or discussions:

1. Check existing [issues](https://github.com/your-org/bos/issues)
2. Review [CONTRIBUTING.md](CONTRIBUTING.md)
3. Open a new issue with clear description

---

## 📚 Additional Resources

- [ReAct Framework](https://arxiv.org/abs/2210.03629) - Reasoning + Acting in Language Models
- [Zenoh](https://zenoh.io/) - Pub/sub routing system
- [Tokio](https://tokio.rs/) - Async Rust runtime

---

**Version**: 0.1.0  
**Edition**: 2021  
**Last Updated**: 2026-03-30
