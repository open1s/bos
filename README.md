<div align="center">

# 🧠 BrainOS

**构建下一代分布式 AI 代理框架**

[![Rust](https://img.shields.io/badge/Rust-1.70+-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Build Status](https://img.shields.io/badge/Build-Passing-green.svg)]()

[快速开始](#-快速开始) • [架构](#-架构概览) • [示例](#-示例) • [文档](#-文档)

</div>

---

## ✨ 特性

BrainOS 是一个高性能、可扩展的分布式 AI 代理框架，专为构建智能协作系统而设计。

### 🚀 核心能力

- **🤖 智能代理框架** - 完整的代理生命周期管理，支持配置驱动和编程式构建
- **🌐 分布式通信** - 基于 Zenoh 的高性能消息总线，支持发布/订阅、RPC 和查询
- **🔌 LLM 集成** - 无缝集成 OpenAI 兼容的 LLM，支持流式响应和函数调用
- **🛠️ 工具系统** - 统一的工具注册和执行机制，支持本地工具和远程 RPC 调用
- **🤝 A2A 协议** - 代理间通信协议，支持任务委托、能力发现和状态管理
- **📊 工作流调度** - DSL 驱动的工作流引擎，支持条件分支、重试和回退策略
- **🔌 MCP 支持** - Model Context Protocol 适配器，连接丰富的工具生态
- **💾 会话管理** - 持久化会话状态，支持序列化和恢复

### 🎯 设计理念

- **类型安全** - 利用 Rust 的类型系统确保编译时安全
- **异步优先** - 基于 Tokio 的全异步架构，高并发性能
- **可组合** - 模块化设计，灵活组合各种功能
- **可观测** - 内置结构化日志和追踪支持

---

## 🚀 快速开始

### 前置要求

- Rust 1.70+
- Zenoh 路由器（可选，用于分布式部署）

### 安装

```bash
# 克隆仓库
git clone https://github.com/your-org/bos.git
cd bos

# 构建项目
cargo build --release
```

### 基础示例

创建一个简单的 AI 代理：

```rust
use agent::{Agent, AgentConfig};
use agent::llm::OpenAiClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 配置 LLM 客户端
    let llm = OpenAiClient::new(
        std::env::var("OPENAI_API_KEY")?,
        std::env::var("OPENAI_API_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        "gpt-4o".to_string(),
    );

    // 构建代理配置
    let config = AgentConfig {
        name: "my-agent".to_string(),
        model: "gpt-4o".to_string(),
        base_url: std::env::var("OPENAI_API_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        api_key: std::env::var("OPENAI_API_KEY")?,
        system_prompt: "你是一个有用的助手。".to_string(),
        temperature: 0.7,
        max_tokens: Some(1000),
        timeout_secs: 60,
    };

    // 创建并运行代理
    let agent = Agent::new(config, Arc::new(llm));
    let response = agent.run("你好！").await?;
    
    println!("{}", response);
    Ok(())
}
```

### 运行示例

```bash
# 启动 Zenoh 路由器（新终端）
zenohd

# 运行 LLM 代理演示
cd examples/llm-agent-demo
export OPENAI_API_KEY="your-key"
cargo run --bin alice
```

---

## 🏗️ 架构概览

```
┌─────────────────────────────────────────────────────────────┐
│                         BrainOS                              │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐          │
│  │   Agent     │  │  Scheduler  │  │   Session   │          │
│  │   Framework │  │   Engine    │  │  Manager    │          │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘          │
│         │                │                │                  │
│  ┌──────▼────────────────▼────────────────▼──────┐          │
│  │              Tool Registry & MCP              │          │
│  └──────┬───────────────────────────────────────┬┘          │
│         │                                       │            │
│  ┌──────▼──────┐                         ┌──────▼──────┐    │
│  │  LLM Client │                         │ A2A Protocol│    │
│  └──────┬──────┘                         └──────┬──────┘    │
│         │                                       │            │
│  ┌──────▼───────────────────────────────────────▼──────┐    │
│  │              Zenoh Communication Bus               │    │
│  └────────────────────────────────────────────────────┘    │
│                                                               │
└─────────────────────────────────────────────────────────────┘
```

### 核心组件

| 组件 | 描述 |
|------|------|
| **Agent Framework** | 代理生命周期管理、消息处理、工具调用 |
| **Bus** | Zenoh 通信抽象，提供 Pub/Sub、RPC、Query |
| **LLM Client** | OpenAI 兼容的 LLM 客户端，支持流式响应 |
| **Tool System** | 工具注册、验证、执行，支持本地和远程 |
| **A2A Protocol** | 代理间通信，任务委托和能力发现 |
| **Scheduler** | 工作流引擎，DSL 定义和执行 |
| **MCP Adapter** | Model Context Protocol 集成 |
| **Session Manager** | 会话状态持久化和恢复 |

---

## 📚 示例

### LLM 代理演示

完整的代理生命周期演示，包含真实 LLM、工具和 A2A 通信。

```bash
cd examples/llm-agent-demo

# 终端 1: 启动计算器代理
cargo run --bin bob

# 终端 2: 启动对话代理
export OPENAI_API_KEY="your-key"
cargo run --bin alice
```

### 基础通信

两个代理通过 A2A 协议交换消息。

```bash
cd examples/basic-communication
cargo run --bin agent1
cargo run --bin agent2
```

### 工作流调度

使用 DSL 定义和执行复杂工作流。

```bash
cd examples/demo-scheduler
cargo run --bin main
```

更多示例请查看 [examples/](examples/) 目录。

---

## 📖 文档

- [API 文档](https://docs.rs/bos) - 完整的 API 参考
- [示例指南](examples/README.md) - 详细的示例说明
- [架构设计](docs/architecture.md) - 系统架构和设计决策
- [贡献指南](CONTRIBUTING.md) - 如何参与贡献

---

## 🛠️ 开发

### 构建项目

```bash
# 开发构建
cargo build

# 发布构建
cargo build --release

# 运行测试
cargo test

# 运行示例
cargo run --example <example-name>
```

### 代码规范

```bash
# 格式化代码
cargo fmt

# 检查代码
cargo clippy

# 运行所有检查
cargo check && cargo clippy && cargo test
```

---

## 🤝 贡献

我们欢迎所有形式的贡献！

1. Fork 本仓库
2. 创建特性分支 (`git checkout -b feature/amazing-feature`)
3. 提交更改 (`git commit -m 'Add amazing feature'`)
4. 推送到分支 (`git push origin feature/amazing-feature`)
5. 开启 Pull Request

请阅读 [CONTRIBUTING.md](CONTRIBUTING.md) 了解详细的贡献指南。

---

## 📄 许可证

本项目采用 MIT 许可证 - 详见 [LICENSE](LICENSE) 文件。

---

## 🙏 致谢

- [Zenoh](https://zenoh.io/) - 高性能数据总线
- [Tokio](https://tokio.rs/) - 异步运行时
- [OpenAI](https://openai.com/) - LLM API

---

<div align="center">

**用 ❤️ 构建智能未来**

[⬆ 回到顶部](#-brainos)

</div>