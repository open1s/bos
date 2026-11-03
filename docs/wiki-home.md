# BOS — Rust 原生 AI Agent 平台

**一个 Runtime，多种开发接口，一个可扩展的 Agent 工作空间。**

使用同一套 Rust Runtime，通过 **Rust、Python 或 JavaScript** 构建 AI Agent；也可以在原生桌面工作空间 **BSH** 中开发和运行 Agent。

**[快速开始](https://github.com/open1s/bos#quick-start) · [查看源码](https://github.com/open1s/bos) · [加入讨论](https://github.com/open1s/bos/discussions)**

---

## 从一个 Runtime 开始，选择你熟悉的开发方式

BOS 将共享的 Agent 执行核心与多语言接口、桌面工作空间结合起来。无论你在构建应用、服务，还是探索 Agent 工作流，都可以从适合自己的入口开始。

| 你想做什么 | 使用 BOS 的方式 |
|---|---|
| 在 Python 项目中构建 Agent | 使用 [NBOS](https://pypi.org/project/nbos) |
| 在 Node.js 项目中构建 Agent | 使用 [JSBOS](https://www.npmjs.com/package/@open1s/jsbos) |
| 将 Agent Runtime 集成到 Rust 应用 | 使用 BOS Rust crates |
| 在桌面环境中开发和运行 Agent | 使用 [BSH](https://github.com/open1s/bos) |
| 为工作空间添加能力 | 开发 [BSH 扩展](https://github.com/open1s/bos/blob/main/docs/bsh-extension-dev-guide.md) |

## 为什么选择 BOS？

- **统一执行核心**：Rust、Python 和 JavaScript 接口共享同一套 Rust Runtime。
- **构建工具型 Agent**：支持 ReAct 执行、工具调用、流式响应和生命周期 Hooks。
- **连接模型与工具生态**：支持多种模型 Provider，并可接入 MCP Server。
- **支持 Agent 协作**：消息总线提供发布/订阅、请求/响应和 RPC 模式。
- **从代码到工作空间**：BSH 将 Agent 与终端、浏览器、编辑器和扩展能力整合在原生桌面环境中。

## 几分钟开始：Python

安装 NBOS：

```bash
pip install nbos
```

创建一个带工具的 Agent：

```python
from nbos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").register(add)
    print(await agent.ask("What is 2 + 2?"))
```

继续阅读：[Python 使用指南](https://github.com/open1s/bos/blob/main/docs/python-user-guide.md)

## BOS 平台组成

- **BOS Runtime** — 共享的 Rust Agent 执行核心
- **NBOS** — Python 接口
- **JSBOS** — JavaScript / Node.js 接口
- **BSH** — 基于 GPUI 构建的原生 Agent 工作空间
- **Extension Platform** — 为 BSH 增加工具、命令、面板等能力

## 下一步

- [快速开始与项目介绍](https://github.com/open1s/bos)
- [架构说明](https://github.com/open1s/bos/blob/main/ARCHITECTURE.md)
- [Rust 使用指南](https://github.com/open1s/bos/blob/main/docs/rust-user-guide.md)
- [Python 使用指南](https://github.com/open1s/bos/blob/main/docs/python-user-guide.md)
- [JavaScript 使用指南](https://github.com/open1s/bos/blob/main/docs/javascript-user-guide.md)
- [BSH 扩展开发指南](https://github.com/open1s/bos/blob/main/docs/bsh-extension-dev-guide.md)
- [版本与更新](https://github.com/open1s/bos/releases)
- [问题反馈](https://github.com/open1s/bos/issues) · [社区讨论](https://github.com/open1s/bos/discussions)

如果 BOS 对你有帮助，欢迎给仓库点 Star、分享你的使用案例，或参与讨论和贡献。

---

## English

**One Runtime. Multiple Interfaces. One Extensible Agent Workspace.**

Build AI agents with Rust, Python, or JavaScript on the same Rust runtime. Or develop and run them in BSH, a native desktop agent workspace.

**[Get started](https://github.com/open1s/bos#quick-start) · [Explore the source](https://github.com/open1s/bos) · [Join the discussion](https://github.com/open1s/bos/discussions)**

BOS brings together a shared agent runtime, multi-language developer interfaces, model and MCP integrations, and an extensible desktop workspace.

- [Python SDK: NBOS](https://pypi.org/project/nbos)
- [JavaScript SDK: JSBOS](https://www.npmjs.com/package/@open1s/jsbos)
- [Architecture](https://github.com/open1s/bos/blob/main/ARCHITECTURE.md)
- [Documentation](https://github.com/open1s/bos#documentation)
- [Releases](https://github.com/open1s/bos/releases)
- [Contribute](https://github.com/open1s/bos/blob/main/CONTRIBUTING.md)
