中文 | [English](./README.md)
# BrainOS (BOS)

一个模块化的 AI 运行时框架，用于构建智能 AI 应用，支持多代理协调、事件流和可扩展工具系统。

## 核心特性

- **🤖 代理框架**: 集成 LLM 的多代理协调，支持技能管理
- **🚌 事件总线**: 高性能发布/订阅消息，支持查询/响应、RPC 模式
- **⚙️ 配置管理**: 支持 TOML、YAML、环境变量的灵活配置加载
- **🧠 ReAct 引擎**: 推理 + 行动循环脚手架，用于 AI 代理工作流
- **🐍 Python 绑定**: `pip install brainos` - 统一高级 Python API
- **📦 Node.js 绑定**: `npm install @open1s/jsbos` - 统一高级 JavaScript API
- **🔄 内存持久化**: 跨会话内存支持
- **🔌 MCP 客户端**: 连接 Model Context Protocol 服务器
- **📚 技能系统**: 从目录定义加载代理能力

---

## 快速开始

### Python (brainos)

```python
from brainos import BrainOS, tool

@tool("两数相加")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").register(add)
    result = await agent.ask("2+2 等于多少?")
```

### JavaScript (@open1s/jsbos / brainos-js)

```javascript
const { BrainOS, tool } = require('@open1s/jsbos/brainos');

class AddTool {
  @tool('两数相加')
  add(a, b) {
    return a + b;
  }
}

const brain = new BrainOS();
await brain.start();
const agent = brain.agent('assistant').withTools(new AddTool());
const result = await agent.ask('2+2 等于多少?');
```

### Rust (agent crate)

```rust
use agent::{Agent, AgentConfig};

let config = AgentConfig::default().name("assistant");
let agent = Agent::builder().config(config).build()?;
let result = agent.run_simple("Hello").await?;
```

---

## 技能系统

代理可以从目录定义加载能力：

```python
# 创建技能目录并注册
skills_dir = "/path/to/skills"
agent.register_skills_from_dir(skills_dir)
```

### 技能格式

每个技能是一个目录，包含 `SKILL.md` 文件和 YAML frontmatter：

```
skills/
├── python-coding/
│   └── SKILL.md
├── api-design/
│   └── SKILL.md
└── database-ops/
    └── SKILL.md
```

**SKILL.md 格式：**
```markdown
---
name: python-coding
description: 项目 Python 编码规范
category: coding
version: 1.0.0
---

# Python 编码规范

你的技能说明内容...
```

LLM 会在系统提示词中收到可用技能列表，并可调用 `load_skill` 获取完整指令。

---

## 项目结构

```
bos/
├── crates/
│   ├── agent/          # AI 代理框架，集成 LLM、技能、工具、MCP
│   ├── bus/            # 发布/订阅、查询/响应、RPC 消息
│   ├── config/         # TOML/YAML 配置加载
│   ├── logging/        # 追踪和可观测性
│   ├── react/          # ReAct 推理引擎
│   ├── pybos/          # Python 绑定 (brainos 包)
│   │   └── brainos/    # 高级 Python 包装器
│   └── jsbos/          # Node.js 绑定 (@open1s/jsbos)
│       └── brainos.js  # 高级 JavaScript 包装器
├── docs/               # 用户指南
│   ├── python-user-guide.md
│   ├── javascript-user-guide.md
│   └── rust-user-guide.md
└── Cargo.toml          # 工作空间
```

---

## 核心 Crate

| Crate | 描述 | 安装 |
|-------|------|------|
| `agent` | 核心代理，集成 LLM、工具、技能、MCP | `cargo add agent` |
| `bus` | 发布/订阅、查询/响应、RPC 消息 | `cargo add bus` |
| `config` | 从 TOML、YAML、环境变量加载配置 | `cargo add config` |
| `logging` | 追踪和可观测性 | `cargo add logging` |
| `react` | ReAct 推理 + 行动引擎 | `cargo add react` |
| `pybos` | Python 绑定 | `pip install brainos` |
| `jsbos` | Node.js 绑定 | `npm install @open1s/jsbos` |

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

# Python 绑定（底层 pybos）
cd crates/pybos && maturin develop

# Node.js 绑定（底层 jsbos）
cd crates/jsbos && npm install && npm run build
```

---

## 用户指南

- **Python**: [docs/python-user-guide.md](docs/python-user-guide.md)
- **JavaScript**: [docs/javascript-user-guide.md](docs/javascript-user-guide.md)
- **Rust**: [docs/rust-user-guide.md](docs/rust-user-guide.md)
- **English**: [README.md](../README.md)

---

## 统一 API

`brainos` 包（Python）和 `@open1s/jsbos/brainos.js`（JavaScript）提供一致的高级 API：

| 功能 | Python | JavaScript |
|------|--------|------------|
| 导入 | `from brainos import BrainOS, tool` | `const { BrainOS, tool } = require('@open1s/jsbos/brainos')` |
| 创建 brain | `async with BrainOS() as brain:` | `const brain = new BrainOS(); await brain.start()` |
| 创建代理 | `brain.agent("name")` | `brain.agent("name")` |
| 链式配置 | `.with_model("gpt-4")` | `.withModel("gpt-4")` |
| 注册工具 | `.register(tool)` | `.withTools(toolDef)` |
| 运行 | `await agent.ask("...")` | `await agent.ask("...")` |
| 总线工厂 | `BusManager()` | `BusManager.create()` |

### 底层绑定

直接访问 Rust 绑定：

| 语言 | 包 | 导入 |
|------|-----|------|
| Python | `pybos` | `from pybos import Agent, Bus, ...` |
| JavaScript | `@open1s/jsbos` | `const { Agent, Bus } = require('@open1s/jsbos')` |

---

## 配置

创建 `~/.bos/conf/config.toml`:

```toml
[global_model]
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"
model = "gpt-4"

[bus]
mode = "peer"
listen = ["127.0.0.1:7890"]
```

或使用环境变量：`OPENAI_API_KEY`、`LLM_BASE_URL`、`LLM_MODEL`

---

## 示例

查看示例目录：

- Python: `crates/pybos/examples/`
- JavaScript: `crates/jsbos/examples/`
- Rust: `crates/examples/` (包含 `agent_skill_demo.rs`)

---

## 许可证

MIT OR Apache-2.0

---

**版本**: 2.0.0 | **更新日期**: 2026-05-06