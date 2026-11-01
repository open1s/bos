<p align="right">
  <a href="README.md">English</a> · <b>中文</b>
</p>

<p align="center">
  <img src="docs/assets/bos-logo.png" alt="BOS — Rust 原生 AI Agent 平台" width="440">
</p>

<h1 align="center">BOS — Rust 原生 AI Agent 平台</h1>

<p align="center">
  <strong>One Runtime. Multiple Interfaces. One Extensible Agent Workspace.</strong><br>
  一个运行时，多种开发接口，一个可扩展的 Agent 工作空间。
</p>

<p align="center">
  BOS（BrainOS）是一个 Rust 原生 AI Agent 平台：以统一的 <strong>BOS Runtime</strong> 为执行核心，
  通过多种开发接口向不同语言和应用开放同一套运行能力，并以 <strong>BSH（BOS Harness）</strong>
  提供可扩展的原生桌面 Agent 工作空间。开发者可以直接使用 Rust，也可以通过 NBOS（Python）
  和 JSBOS（JavaScript）接入同一运行时。
</p>

<p align="center">
  <a href="https://pypi.org/project/nbos"><img src="https://img.shields.io/pypi/v/nbos?label=Python%20SDK&logo=pypi&color=3776AB" alt="Python SDK"></a>
  <a href="https://www.npmjs.com/package/@open1s/jsbos"><img src="https://img.shields.io/npm/v/@open1s/jsbos?label=JavaScript%20SDK&logo=npm&color=CB3837" alt="JavaScript SDK"></a>
  <a href="https://github.com/open1s/bos/actions/workflows/rust.yml"><img src="https://github.com/open1s/bos/actions/workflows/rust.yml/badge.svg" alt="Rust CI"></a>
  <a href="https://github.com/open1s/bos/actions/workflows/binding-parity.yml"><img src="https://github.com/open1s/bos/actions/workflows/binding-parity.yml/badge.svg" alt="Binding parity"></a>
  <a href="https://img.shields.io/github/license/open1s/bos"><img src="https://img.shields.io/github/license/open1s/bos" alt="License"></a>
  <a href="docs/BOS-BSH-Textbook.md"><img src="https://img.shields.io/badge/docs-34%20章实战手册-blue" alt="文档"></a>
</p>

<p align="center">
  <a href="#快速开始">快速开始</a> ·
  <a href="#为什么选择-bos">产品特点</a> ·
  <a href="#bsh原生-ai-agent-工作空间">BSH 桌面端</a> ·
  <a href="#扩展平台">扩展平台</a> ·
  <a href="#文档">文档</a>
</p>

---

## BOS 是什么？

**BOS 的核心是一套共享的 Rust Runtime，通过多种开发接口开放能力，并最终汇聚到一个可扩展的 Agent 工作空间。**

Runtime 是统一执行基础，不依附于某一种语言 SDK 或桌面应用。开发者可以直接使用 Rust，
也可以通过 NBOS（Python）和 JSBOS（JavaScript）调用同一套执行内核。BSH 则是建立在
该 Runtime 之上的原生 GPUI 工作空间，将 Agent 执行、开发工具与扩展平台整合在一起。

| 产品组成 | 核心能力 | 适用场景 |
|---|---|---|
| **BOS Runtime** | Rust Agent 引擎、工具、Skills、Memory、MCP Client、多智能体总线、策略与可观测性 | 将 Agent 能力嵌入自己的应用和服务 |
| **NBOS** | BOS 内核的 Python 绑定 | Python 应用、自动化和快速原型 |
| **JSBOS** | BOS 内核的 Node.js / JavaScript 绑定 | Node.js 服务、开发者工具和 JavaScript 自动化 |
| **BSH** | 原生桌面 AI Agent 工作空间，集成对话、终端、浏览器、编辑器和扩展 | 希望在统一环境中构建、使用和管理智能体的开发者 |

## 为什么选择 BOS？

BOS 的核心设计原则是：**Agent Runtime 不应被某一种语言、某一个应用或某一种界面锁定。**
同一套 Rust 实现同时服务于原生 Rust 应用、Python 与 JavaScript SDK，以及 BSH 桌面端。
团队因此可以使用一致的执行模型，将 Agent 嵌入业务产品、通过脚本自动化，或在桌面工作空间中运行。

- **多种接口，一套执行模型。** 可以直接使用 Rust Runtime，也可以通过 NBOS Python
  和 JSBOS JavaScript SDK 开发。语言绑定共享底层实现，并通过 CI 检查 API 一致性。
- **完整 Agent 执行循环。** 基于 ReAct 完成模型流式输出、工具调用、结果观察和后续推理，
  并提供生命周期 Hooks。
- **内置 MCP。** 通过 stdio 或 HTTP 连接 Model Context Protocol 服务端，发现并调用工具，
  访问资源与提示模板。
- **多智能体通信。** 提供发布/订阅、请求/响应以及 RPC，不必为 Agent 间协作另造一套通信层。
- **Skills 与 Memory。** 按需加载目录化 `SKILL.md` 知识，并使用内存或文件型记忆保留上下文。
- **运行治理能力。** 支持重试分类、限流、熔断、执行期限、Token/工具调用指标和能力授权。
- **面向扩展设计。** 可将 BOS 嵌入自己的应用，也可以通过声明式、Process、WASM Component、
  Extism、MCP 和 Built-in 等方式扩展 BSH。

## 快速开始

### Python

```bash
pip install nbos
```

```python
from nbos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").with_tools(add)
    print(await agent.ask("What is 2 + 2?"))
```

### JavaScript / Node.js

```bash
npm install @open1s/jsbos
```

```javascript
import { BrainOS, ToolDef } from "@open1s/jsbos";

const add = new ToolDef(
  "add",
  "Add two numbers",
  (args) => args.a + args.b,
  { type: "object", properties: { result: { type: "number" } }, required: ["result"] },
  { type: "object", properties: { a: { type: "number" }, b: { type: "number" } }, required: ["a", "b"] },
);

const brain = new BrainOS();
await brain.start();
const agent = await brain.agent("assistant").register(add).start();
console.log(await agent.runSimple("What is 2 + 2?"));
```

### Rust

从 workspace 中引入需要的 crate，直接使用 Rust 运行时：

```rust
use agent::{Agent, AgentConfig};

#[tokio::main]
async fn main() {
    let config = AgentConfig::default()
        .name("assistant")
        .model("openai/gpt-4o")
        .api_key(std::env::var("OPENAI_API_KEY").unwrap_or_default());

    let agent = Agent::from_config(config);
    println!("{}", agent.run_simple("Hello").await.expect("agent run"));
}
```

### 运行 BSH 桌面应用

```bash
git clone https://github.com/open1s/bos
cd bos
cargo build --release -p bsh
./target/release/bsh
```

BSH 需要 Rust 1.97+ 和 Zig 0.16（终端面板需要构建 Ghostty）。
macOS 可通过 `./crates/bsh/make-dev-app.sh` 打包开发版；
Linux 构建依赖请参考 [Rust CI workflow](.github/workflows/rust.yml)。

## 模型与 Provider 配置

BOS 支持 OpenAI 兼容接口、Anthropic 协议、NVIDIA NIM、Google AI 和 OpenRouter。
在 `~/.bos/conf/config.toml` 中配置默认模型：

```toml
[global_model]
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"
model = "gpt-4"

[llm.nvidia]
api_key = "nv-..."
base_url = "https://integrate.api.nvidia.com/v1"
model = "nvidia/your-model"
```

如果调用参数与配置文件都没有提供 key，两种绑定都会读取 `OPENAI_API_KEY`；
没有 `LLM_BASE_URL`、`LLM_MODEL` 这类覆盖——base URL 与模型来自调用参数或上面的配置段。

BSH 可在 **设置 → 模型** 中管理 Provider。敏感凭据存放在加密 Vault 中，
不会以明文形式写入普通设置文件。

---

## BOS 的设计方式

| 设计原则 | BOS 的实现 |
|---|---|
| **统一执行内核** | 一套 Rust Runtime 同时服务于原生 Rust 应用、NBOS、JSBOS 和 BSH。 |
| **Agent 是执行流程，不只是对话** | ReAct 循环负责协调模型输出、工具调用、结果观察与生命周期 Hooks。 |
| **默认支持互操作** | 通过 stdio 和 HTTP 接入 MCP Server，并通过 Provider 适配器连接模型 API。 |
| **多 Agent 协作** | 专用消息总线提供发布/订阅、请求/响应和 RPC。 |
| **知识与上下文可复用** | 目录化 `SKILL.md` 技能，以及内存或文件型 Memory。 |
| **执行过程可治理** | 重试策略、限流、熔断、执行期限、指标和能力授权。 |
| **运行时可持续扩展** | BSH 支持 Declarative、Process、WASM Component、Extism、MCP 和 Built-in 扩展。 |

BOS 将安全边界视为需要明确实现和验证的工程能力，而不是营销承诺。
沙箱能力取决于操作系统与执行路径；BSH Process/MCP 扩展在 Windows 上的
AppContainer 隔离和 Job Object 资源限制仍在跟踪实施中
（[#43](https://github.com/open1s/bos/issues/43)、[#39](https://github.com/open1s/bos/issues/39)）。

---

## 深入 BOS Runtime

- **智能体循环**——`crates/react` 跑 ReAct：流式输出、调用工具、观察、继续。各条缝上都有
  hook（`before_tool_call`、`after_llm_call`、`on_complete`、`on_error` ……），宿主可以观察
  或拦截某一步，而不必 fork 引擎。
- **供应商**——任何 OpenAI 兼容端点、Anthropic 协议、NVIDIA NIM、Google AI 与 OpenRouter，
  按配置或按模型前缀选择。
- **工具**——用 `FunctionTool`/`AsyncFunctionTool` 包一个闭包，或直接实现 `Tool`/`AsyncTool`
  再注册；智能体通过 `react` 的缝调用它们，永远不经过总线。
- **技能**——指向一个装满 `SKILL.md` 的目录，模型会在系统提示词里拿到目录，再调用
  `load_skill` 取完整指令。
- **记忆**——`InMemoryMemory` 按关键词重叠与新旧排序；`FileMemory` 把同样的排序写进
  JSON lines，重启后仍在。过滤器限定召回范围，`{{memory}}` 把它放进提示词。
- **MCP**——接入你已有的服务器（stdio 或 HTTP），把它们的工具、资源与提示词当成原生能力用。
- **总线**——`Publisher`/`Subscriber` 做事件，`Queryable` 做请求/响应，`Callable`/`Caller` 做
  RPC；一个 session 把它们挂到同一条传输上。
- **沙箱**——`heel` 是唯一的沙箱实现：工作区是唯一可写处，可执行字节无处落盘，读取在基线
  之上需显式开启，网络走策略。
- **可观测性**——`crates/logging` 的 tracing，加上调用指标（token、工具调用、墙钟时间、
  限流等待），两种绑定都能读到。

### MCP，不需要再写一层封装

```python
from nbos import McpClient

client = await McpClient.spawn("npx", ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"])
await client.initialize()
```

## Runtime 核心能力

| 能力 | 说明 |
|---|---|
| **Agent Loop** | ReAct 执行、流式输出、工具调用、结果观察和生命周期 Hooks |
| **LLM Providers** | OpenAI 兼容 API、Anthropic、NVIDIA NIM、Google AI、OpenRouter |
| **Tools** | 同步/异步工具、自定义 Tool Trait、JSON Schema 定义 |
| **MCP** | stdio 与 HTTP Client；支持 tools、resources、prompts |
| **Multi-Agent Bus** | 发布/订阅、请求/响应和 RPC |
| **Skills** | 从 `SKILL.md` 目录按需加载专业知识 |
| **Memory** | 内存与文件型记忆、元数据过滤、Prompt 注入 |
| **Resilience** | 重试分类、限流、熔断和执行期限 |
| **Observability** | Tracing，以及每次运行的 Token、工具调用、耗时和限流指标 |

### 一套 Runtime，两种一致的 SDK

NBOS 和 JSBOS 是同一原生引擎的语言接口，而不是两套独立重写的实现。
CI 会检查公开 API、Native Wrapper 映射和 API 文档，避免绑定接口逐渐分叉。

| | Python | JavaScript |
|---|---|---|
| 导入 | `from nbos import BrainOS, tool` | `import { BrainOS, ToolDef } from "@open1s/jsbos"` |
| Runtime | `async with BrainOS() as brain:` | `const brain = new BrainOS(); await brain.start()` |
| 创建 Agent | `brain.agent("assistant")` | `brain.agent("assistant")` |
| 注册工具 | `.with_tools(add)` | `.register(toolDef)` |
| 执行 | `await agent.ask("…")` | `await agent.runSimple("…")` |

## 一个可扩展的 Agent 工作空间：BSH

**BSH 是建立在 BOS Runtime 之上的原生 GPUI 工作空间。** 它将 Agent 会话、项目、终端、
浏览器、代码编辑器和扩展整合到同一环境。Agent 执行能力与开发者日常使用的工具并置，
扩展平台则允许团队围绕自己的工作流程定制工作空间。

- **工作区与会话** —— 按项目组织工作空间和持久会话，支持搜索、置顶、归档、分叉与恢复。
- **Agent 对话** —— 流式响应、轮次导航、任务计划、消息队列、子 Agent 运行状态和定时任务。
- **集成终端** —— 内嵌 Ghostty 终端，并在命令执行前应用终端策略检查。
- **浏览器面板** —— 在 Agent 工作空间中使用嵌入式 WebView。
- **代码编辑器** —— 文件树、标签页、Diff 和 tree-sitter 语言语法。
- **加密设置** —— Provider 配置与加密 Vault，使用 Argon2id 和 XChaCha20-Poly1305。
- **双语界面** —— 支持中文与英文。
- **扩展平台** —— 增加工具、命令、面板、提示词、上下文、技能、主题、语言服务、Agent 角色、
  工作流、存储和 Hooks。

## 扩展平台

扩展平台是 BSH 工作空间的可扩展层。BOS Runtime 面向应用集成，BSH 则围绕开发者
工作流程进行扩展。每个扩展通过 `extension.toml` 描述和打包；BSH 负责校验 Manifest 与
包内容、解析依赖、管理生命周期，并在扩展请求宿主能力时执行相应的能力授权策略。

<p align="center">
  <img src="docs/assets/charts/architecture.png" alt="BSH 扩展架构：校验、哈希、安装、依赖解析、授权、分发与执行" width="820">
</p>

### 六种扩展运行时

| Runtime | 适用场景 | 扩展实现 |
|---|---|---|
| **Declarative** | JSON 定义工具、面板、提示词和主题 | 声明配置，不执行 Guest 代码 |
| **Process** | 复用已有程序和 CLI | 通过行分隔 JSON-RPC 通信的子进程 |
| **WASM Component** | 具有类型化接口的可移植组件 | WebAssembly Component Model 与 WIT |
| **Extism** | 接入现有 Extism 插件 | 使用 `bsh_host` 导入接口的 Extism 插件 |
| **MCP** | 复用已有 MCP Server | stdio MCP Server |
| **Built-in** | 第一方内置能力 | 链接进宿主的 Rust 代码 |

<p align="center">
  <img src="docs/assets/charts/call-path.png" alt="扩展调用链：能力授权、策略决策、审计与分发执行" width="700">
</p>

### 创建和验证扩展

```bash
export BSH=./target/release/bsh

$BSH ext new com.acme.hello --dir hello
$BSH ext validate hello
$BSH ext mock hello echo '{"text":"hi"}'
$BSH ext install hello
$BSH ext list
$BSH ext doctor
$BSH ext grant com.acme.hello workspace.write --scope workspace
$BSH ext logs com.acme.hello
```

查看 [`examples/extensions/`](examples/extensions/) 中的示例：
[`hello`](examples/extensions/hello)、
[`process-echo`](examples/extensions/process-echo)、
[`wasm-echo`](examples/extensions/wasm-echo)、
[`extism-echo`](examples/extensions/extism-echo)、
[`mcp-echo`](examples/extensions/mcp-echo) 和
[`extism-hook`](examples/extensions/extism-hook)。

扩展开发资料：

- [扩展标准](docs/bsh-extension-standard.md) —— Manifest、能力与运行时的规范契约。
- [扩展开发指南](docs/bsh-extension-dev-guide.md) —— 从零创建第一个扩展。
- [WIT 接口](crates/bsh/wit/extension.wit) —— WASM Component 类型化接口。
- [Hook 系统](docs/bsh-hook-system.md) —— 生命周期与观测 Hook。
- [CI 验证脚本](scripts/ci/verify-bsh-examples.sh) —— 扩展示例验证入口。

**安全说明：**扩展能力授权与操作系统级隔离是两个不同层次。
不同操作系统和运行时可提供的沙箱能力并不完全相同；Windows 上 Process 和 MCP
扩展子进程的隔离工作仍在单独跟踪。详见
[扩展安全规范](docs/bsh-extension-standard.md) 和
[未关闭的安全问题](https://github.com/open1s/bos/issues)。

## 仓库结构

```
crates/
├── agent/        Agent Runtime、工具、Skills、Memory、MCP、策略
├── react/        ReAct 引擎与 LLM Client
├── bus/          发布/订阅、请求/响应与 RPC
├── config/       配置加载与发现
├── logging/      Tracing 与埋点
├── qserde/       总线使用的 rkyv 序列化
├── resource/     资源子系统与 rex CLI
├── bsh/          原生桌面 Harness 与扩展平台
├── nbos/         Python 绑定（pip install nbos）
└── jsbos/        JavaScript 绑定（npm install @open1s/jsbos）
```

组件关系与依赖方向见 [ARCHITECTURE.md](ARCHITECTURE.md)。

## 文档

| 资料 | 内容 |
|---|---|
| [BOS / BSH 实战手册](docs/BOS-BSH-Textbook.md) | 34 章文档，覆盖 Runtime、桌面应用和扩展平台 |
| [Python 使用指南](docs/python-user-guide.md) | 在 Python 中使用 BOS |
| [JavaScript 使用指南](docs/javascript-user-guide.md) | 在 Node.js / JavaScript 中使用 BOS |
| [Rust 使用指南](docs/rust-user-guide.md) | 嵌入和扩展 Rust Runtime |
| [扩展标准](docs/bsh-extension-standard.md) · [开发指南](docs/bsh-extension-dev-guide.md) | 扩展规范与开发流程 |
| [架构](ARCHITECTURE.md) · [项目上手](ONBOARDING.md) | 系统设计与贡献者环境 |
| [设置](docs/bsh-settings-spec.md) · [终端策略](docs/bsh-terminal-policy.md) · [Hooks](docs/bsh-hook-system.md) | 子系统设计说明 |
| [REX 指南](docs/rex-user-guide.md) · [诊断](docs/diagnostics.md) | 资源 CLI 与故障排查 |
| [Wiki](https://github.com/open1s/bos/wiki) · [更新日志](CHANGELOG.md) | 使用指南与版本记录 |

## 构建、测试与贡献

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

Python 和 JavaScript 绑定有独立测试套件：

```bash
cd crates/nbos  && maturin develop && pytest -m "not llm"
cd crates/jsbos && npm install && npx ava
```

- [贡献指南](CONTRIBUTING.md)
- [安全策略](SECURITY.md)
- [GitHub Issues](https://github.com/open1s/bos/issues)
- [适合新贡献者的 Issues](https://github.com/open1s/bos/labels/good%20first%20issue)

如果 BOS 对你有帮助，欢迎给仓库点一个 **Star**，也欢迎分享你基于 BOS 构建的项目。
Star、Issue、Discussion 和真实使用案例，都能帮助更多开发者发现 BOS，也能帮助我们确定后续优先级。

## License

MIT OR Apache-2.0 · Version 3.0.2 · [Releases](https://github.com/open1s/bos/releases)
