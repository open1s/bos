# BrainOS (BOS)

一个模块化的 AI 运行时框架，用于构建智能 AI 应用，支持多代理协调、事件流和可扩展工具系统。

## 核心特性

- **🤖 代理框架**: 集成 LLM 的多代理协调，支持技能管理
- **🚌 事件总线**: 高性能发布/订阅消息，支持查询/响应、RPC 模式
- **⚙️ 配置管理**: 支持 TOML、YAML、环境变量的灵活配置加载
- **🧠 ReAct 引擎**: 推理 + 行动循环脚手架，用于 AI 代理工作流
- **🐍 Python 绑定**: `pip install brainos` - 统一 Python API
- **📦 Node.js 绑定**: `npm install brainos` - 统一 JavaScript API
- **🔄 内存持久化**: 跨会话内存支持

---

## 快速开始

### Rust

```rust
use agent::{Agent, AgentConfig};

let config = AgentConfig::default().name("assistant");
let agent = Agent::builder().config(config).build()?;
let result = agent.run_simple("Hello").await?;
```

### Python

```python
from brainos import BrainOS, tool

@tool("两数相加")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").register(add)
    result = await agent.ask("2+2 等于多少?")
```

### JavaScript

```javascript
const { BrainOS, ToolDef } = require('brainos');

const addTool = new ToolDef('add', '两数相加', (args) => args.a + args.b, ...);
const brain = new BrainOS();
await brain.start();
const agent = brain.agent('assistant').register(addTool);
const result = await agent.ask('2+2 等于多少?');
```

---

## 项目结构

```
bos/
├── crates/
│   ├── agent/      # AI 代理框架，集成 LLM、技能、工具
│   ├── bus/        # 发布/订阅、查询/响应、RPC 消息
│   ├── config/     # TOML/YAML 配置加载
│   ├── logging/    # 追踪和可观测性
│   ├── react/      # ReAct 推理引擎
│   ├── pybos/      # Python 绑定 (brainos 包)
│   └── jsbos/      # Node.js 绑定 (brainos 包)
├── docs/           # 用户指南
│   ├── python-user-guide.md
│   ├── javascript-user-guide.md
│   └── rust-user-guide.md
└── Cargo.toml      # 工作空间
```

---

## 核心 Crate

| Crate | 描述 |
|-------|------|
| `agent` | 核心代理，集成 LLM、工具、技能、MCP |
| `bus` | 发布/订阅、查询/响应、RPC 消息 |
| `config` | 从 TOML、YAML、环境变量加载配置 |
| `logging` | 追踪和可观测性 |
| `react` | ReAct 推理 + 行动引擎 |
| `pybos` | Python 绑定 (`pip install brainos`) |
| `jsbos` | Node.js 绑定 (`npm install brainos`) |

---

## 常用命令

```bash
# 构建所有
cargo build --all

# 测试所有
cargo test --all

# 代码检查
cargo clippy --all
cargo fmt --all

# Python 绑定
cd crates/pybos && maturin develop

# Node.js 绑定
cd crates/jsbos && npm install && npm run build
```

---

## 用户指南

- **Python**: [docs/python-user-guide.md](docs/python-user-guide.md)
- **JavaScript**: [docs/javascript-user-guide.md](docs/javascript-user-guide.md)  
- **Rust**: [docs/rust-user-guide.md](docs/rust-user-guide.md)

---

## 统一 API

Python 和 JavaScript API 保持一致：

| 功能 | Python | JavaScript |
|------|--------|------------|
| 创建代理 | `brain.agent("name")` | `brain.agent("name")` |
| 链式配置 | `.with_model("gpt-4")` | `.withModel("gpt-4")` |
| 注册工具 | `.register(tool)` | `.register(toolDef)` |
| 运行 | `await agent.ask("...")` | `await agent.ask("...")` |
| 总线工厂 | `BusManager()` | `BusManager.create()` |

---

## 许可证

MIT OR Apache-2.0

---

**版本**: 0.1.0 | **更新日期**: 2026-04-05