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
- **🔌 MCP 客户端**: 连接 Model Context Protocol 服务器（stdio 和 HTTP）
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
    agent = brain.agent("assistant").with_tools(add)
    result = await agent.ask("2+2 等于多少?")
```

### JavaScript (@open1s/jsbos / brainos-js)

```javascript
const { BrainOS, ToolDef } = require('@open1s/jsbos/brainos');

// 创建工具使用 ToolDef
const addTool = new ToolDef(
  'add',
  '两数相加',
  (args) => (args.a || 0) + (args.b || 0),
  { type: 'object', properties: { result: { type: 'number' } }, required: ['result'] },
  { type: 'object', properties: { a: { type: 'number' }, b: { type: 'number' } }, required: ['a', 'b'] }
);

const brain = new BrainOS();
await brain.start();
const agent = await brain.agent('assistant')
  .register(addTool)
  .start();
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
| Python | `pybos` | `from pybos import Agent, Bus, McpClient, ...` |
| JavaScript | `@open1s/jsbos` | `const { Agent, Bus, McpClient } = require('@open1s/jsbos')` |

---

## MCP 客户端

通过 stdio 或 HTTP 连接到 MCP 服务器：

### Python

```python
from pybos import McpClient

# 基于进程的服务器
client = await McpClient.spawn("npx", ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"])
await client.initialize()

# HTTP 服务器
client = McpClient.connect_http("http://127.0.0.1:8000/mcp")
await client.initialize()

# 使用工具
tools = await client.list_tools()
result = await client.call_tool("echo", '{"text": "hello"}')
```

### JavaScript

```javascript
const { McpClient } = require('@open1s/jsbos');

// 基于进程的服务器
const client = await McpClient.spawn("npx", ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]);
await client.initialize();

// HTTP 服务器
const client = McpClient.connectHttp("http://127.0.0.1:8000/mcp");
await client.initialize();

// 使用工具
const tools = await client.listTools();
const result = await client.callTool("echo", '{"text": "hello"}');
```

### HTTP 服务器示例

```bash
# 启动 MCP HTTP 服务器
python3 crates/examples/mcp_http_server.py
# 服务器运行在 http://127.0.0.1:8000/mcp
```

---

## 配置

创建 `~/.bos/conf/config.toml`:

```toml
[global_model]
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"
model = "gpt-4"

# 或使用 NVIDIA NIM
[global_model]
api_key = "nv-..."
base_url = "https://api.nvidia.com/v"
model = "nvidia/llama-3.1-nemotron-70b-instruct"

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

### MCP 演示

```bash
# JavaScript MCP HTTP 演示
node crates/jsbos/examples/mcp_http_agent_demo.cjs

# Python MCP HTTP 演示（先启动服务器，然后使用）
python3 crates/examples/mcp_http_server.py
```

---

## 许可证

MIT OR Apache-2.0

---

**版本**: 2.0.6 | **更新日期**: 2026-05-10