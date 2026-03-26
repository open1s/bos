# BrainOS Directory Structure

## Workspace Layout

```
bos/
├── Cargo.toml                    # Workspace configuration
├── crates/                       # Workspace members
│   ├── config/                   # Configuration management
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs           # Public API
│   │       ├── loader.rs        # Configuration loading
│   │       ├── types.rs         # Type definitions
│   │       └── error.rs         # Error types
│   │
│   ├── bus/                      # Zenoh communication
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs           # Public API
│   │       ├── publisher.rs     # Message publishing
│   │       ├── subscriber.rs    # Message subscription
│   │       ├── caller.rs        # Query/response client
│   │       ├── callable.rs      # Queryable service
│   │       ├── session.rs       # Zenoh session
│   │       ├── query.rs         # Query types
│   │       ├── queryable.rs     # Queryable trait
│   │       ├── codec.rs         # Serialization
│   │       └── error.rs         # Error types
│   │
│   ├── agent/                    # Core agent infrastructure
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs           # Public API
│   │       ├── error.rs         # Error types
│   │       │
│   │       ├── agent/           # Agent implementation
│   │       │   ├── mod.rs
│   │       │   ├── config.rs    # Agent configuration
│   │       │   ├── agentic.rs   # Agent logic
│   │       │   ├── context.rs   # Agent context
│   │       │   └── message.rs  # Message types
│   │       │
│   │       ├── llm/             # LLM client
│   │       │   ├── mod.rs
│   │       │   └── client.rs    # LLM client implementation
│   │       │
│   │       ├── tools/           # Tool system
│   │       │   ├── mod.rs
│   │       │   ├── registry.rs  # Tool registry
│   │       │   ├── function.rs  # Function tools
│   │       │   ├── validator.rs # Tool validation
│   │       │   └── translator.rs # Tool translation
│   │       │
│   │       ├── skills/          # Skill system
│   │       │   ├── mod.rs
│   │       │   ├── loader.rs    # Skill loading
│   │       │   ├── injector.rs  # Skill injection
│   │       │   ├── metadata.rs  # Skill metadata
│   │       │   └── tests.rs     # Skill tests
│   │       │
│   │       ├── mcp/             # MCP integration
│   │       │   ├── mod.rs
│   │       │   ├── client.rs    # MCP client
│   │       │   ├── adapter.rs   # MCP tool adapter
│   │       │   ├── protocol.rs  # MCP protocol
│   │       │   ├── transport.rs # Transport layer
│   │       │   └── tests.rs     # MCP tests
│   │       │
│   │       ├── session/         # Session management
│   │       │   ├── mod.rs
│   │       │   ├── manager.rs   # Session manager
│   │       │   ├── storage.rs   # Session storage
│   │       │   └── serializer.rs # Session serialization
│   │       │
│   │       └── streaming/       # Streaming support
│   │           └── mod.rs
│   │
│   └── logging/                 # Logging infrastructure
│       ├── Cargo.toml
│       └── src/
│           └── lib.rs          # Logging initialization
│
├── target/                       # Build artifacts
├── .planning/                    # Planning documents
│   └── codebase/                 # Codebase analysis
├── .github/                      # GitHub workflows
├── .vscode/                      # VSCode settings
├── .idea/                        # IntelliJ settings
└── .sisyphus/                    # Sisyphus agent data
```

## Key Locations

### Configuration Files

- `Cargo.toml` - Workspace and crate configurations
- `crates/*/Cargo.toml` - Individual crate configurations

### Source Code

- `crates/*/src/lib.rs` - Public API for each crate
- `crates/*/src/*.rs` - Implementation modules

### Build Artifacts

- `target/` - Compiled binaries and dependencies
- `target/debug/` - Debug builds
- `target/release/` - Release builds

### Documentation

- `.planning/codebase/` - Codebase analysis documents
- `crates/*/src/*.rs` - Inline documentation

## Naming Conventions

### Files

- **Modules**: `snake_case.rs` (e.g., `agent.rs`, `tool_registry.rs`)
- **Tests**: `tests.rs` or `mod.rs` with `#[cfg(test)]` modules
- **Libraries**: `lib.rs` for public API

### Directories

- **Crates**: `kebab-case` (e.g., `agent`, `bus`, `config`)
- **Modules**: `snake_case` (e.g., `agent/`, `tools/`, `mcp/`)

### Code

- **Types**: `PascalCase` (e.g., `Agent`, `ToolRegistry`)
- **Functions**: `snake_case` (e.g., `execute_tool`, `load_skill`)
- **Constants**: `SCREAMING_SNAKE_CASE` (e.g., `MAX_RETRIES`)
- **Private items**: Leading underscore (e.g., `_internal`)

## Module Organization

### Agent Crate (`crates/agent`)

**Purpose**: Core agent infrastructure

**Module Structure**:
- `agent/` - Agent implementation
- `llm/` - LLM client
- `tools/` - Tool system
- `skills/` - Skill system
- `mcp/` - MCP integration
- `session/` - Session management
- `streaming/` - Streaming support

### Bus Crate (`crates/bus`)

**Purpose**: Zenoh communication wrapper

**Module Structure**:
- `publisher.rs` - Message publishing
- `subscriber.rs` - Message subscription
- `caller.rs` - Query/response client
- `callable.rs` - Queryable service
- `session.rs` - Zenoh session
- `query.rs` - Query types
- `queryable.rs` - Queryable trait
- `codec.rs` - Serialization

### Config Crate (`crates/config`)

**Purpose**: Configuration management

**Module Structure**:
- `loader.rs` - Configuration loading
- `types.rs` - Type definitions
- `error.rs` - Error types

### Logging Crate (`crates/logging`)

**Purpose**: Logging infrastructure

**Module Structure**:
- `lib.rs` - Logging initialization

## File Organization Patterns

### Public API

Each crate's `lib.rs` exports:
- Public types
- Public traits
- Public functions
- Re-exports from submodules

### Module Structure

- **Flat modules**: Simple modules in `src/`
- **Nested modules**: Complex functionality in subdirectories
- **Test modules**: `tests.rs` or `#[cfg(test)]` in `mod.rs`

### Dependencies

- **Workspace dependencies**: Defined in workspace `Cargo.toml`
- **Local dependencies**: Use `path = "../crate-name"`
- **External dependencies**: Use version from workspace

## Build Artifacts

### Target Directory

```
target/
├── debug/              # Debug builds
├── release/            # Release builds
├── doc/                # Documentation
└── flycheck0/          # Incremental builds
```

### Log Directory

```
log/
└── bos-*.log           # Log files with rotation
```

## Configuration Files

### Workspace Configuration

- `Cargo.toml` - Workspace members, dependencies, profiles

### Crate Configuration

- `crates/*/Cargo.toml` - Individual crate settings

### Features

- **Python bindings**: `python` feature in config and bus crates
- **Python extension**: `python-extension` feature in bus crate

## Development Files

### IDE Configuration

- `.vscode/` - VSCode settings
- `.idea/` - IntelliJ settings

### CI/CD

- `.github/workflows/` - GitHub Actions workflows

### Agent Data

- `.sisyphus/` - Sisyphus agent runtime data
