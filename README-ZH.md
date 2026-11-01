<p align="right">
  <a href="README.md">English</a> · <b>中文</b>
</p>

<p align="center">
  <img src="docs/assets/bos-logo.png" alt="BOS — BrainOS AI 智能体框架" width="440">
</p>

<h1 align="center">BOS — Rust 原生 AI Agent 运行时</h1>

<p align="center">
  <strong>一套智能体引擎，贯通 Rust、Python、JavaScript，以及原生桌面工作空间。</strong>
</p>

<p align="center">
  BOS（BrainOS）是一套以 Rust 为核心、面向多语言开发的 AI Agent 框架与运行时，
  将 Agent 编排、工具调用、MCP、Skills、Memory 和多智能体通信整合在统一内核中。
  <strong>BSH（BOS Harness）</strong>则是基于同一运行时构建的原生桌面环境，
  把智能体、代码、终端、浏览器和扩展放进一个工作空间。
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

**BOS 不只是 Prompt 封装，而是一套可嵌入、可扩展、可跨语言复用的 AI Agent Runtime。**

它为应用提供统一的智能体基础设施：Rust 执行内核、Python 与 JavaScript SDK、
一致的工具和 MCP 接口、可持续使用的上下文，以及多智能体通信原语。
你可以把 BOS 作为库嵌入自己的产品，也可以直接使用 BSH，在原生桌面应用中构建和运行智能体。

| 产品组成 | 核心能力 | 适用场景 |
|---|---|---|
| **BOS Runtime** | Rust Agent 引擎、工具、Skills、Memory、MCP Client、多智能体总线、策略与可观测性 | 将 Agent 能力嵌入自己的应用和服务 |
| **NBOS** | BOS 内核的 Python 绑定 | Python 应用、自动化和快速原型 |
| **JSBOS** | BOS 内核的 Node.js / JavaScript 绑定 | Node.js 服务、开发者工具和 JavaScript 自动化 |
| **BSH** | 原生桌面 AI Agent 工作空间，集成对话、终端、浏览器、编辑器和扩展 | 希望在统一环境中构建、使用和管理智能体的开发者 |

## 为什么选择 BOS？

很多 Agent 框架绑定单一语言，或者把模型、工具、MCP 和记忆拼成松散的集成；
桌面 Agent 工具又往往无法与业务应用共享同一套运行时。
BOS 选择另一条路线：**一个 Rust 内核、多语言 SDK，以及基于同一引擎构建的桌面 Harness。**

- **一个内核，三种语言。** 直接使用 Rust，或通过 Python、JavaScript SDK 开发。
  两种语言绑定共享底层实现，并通过 CI 检查 API 一致性。
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

也可以用环境变量：`OPENAI_API_KEY`、`LLM_BASE_URL`、`LLM_MODEL`。

**BSH** 把同样的供应商做成了可编辑的行：**设置 → 模型**。它的设置存放在
`~/.bos/bsh/settings.toml`；不该以明文落盘的凭据，放进它旁边的加密保险库。

---

## 为什么不用另一个智能体框架

| 你需要的 | 这里有什么 | 常见的替代 |
|---|---|---|
| **一份行为，三种语言** | 一套 Rust 内核，上面是 `nbos` 与 `@open1s/jsbos`；两边接口一旦漂移，CI 直接失败 | 一个 Python 框架，加一个半途而废的 SDK |
| **智能体之间能对话** | `bus`：发布/订阅、查询/响应、caller/callable RPC，rkyv 线格式 | 进程之间拼 HTTP |
| **工具真的能跑** | `heel` 为宿主管理的命令提供原生 OS 隔离；BSH 扩展子进程另有隔离边界（macOS profile、Linux Bubblewrap）。Windows Process/MCP 目前没有 AppContainer 隔离（[#43](https://github.com/open1s/bos/issues/43)），Job Object 资源限制也尚未实现（[#39](https://github.com/open1s/bos/issues/39)）。 | “模型都这么说了，应该没事” |
| **领域知识** | 目录式技能：带 frontmatter 的 `SKILL.md`，由模型按需加载 | 一段越写越长的系统提示词 |
| **外部工具服务** | MCP 客户端，stdio 与 HTTP 都支持，工具/资源/提示词一并映射 | 每个服务器手写一个适配器 |
| **上下文记得住** | 内存与文件两种记忆后端，支持元数据过滤，`{{memory}}` 直接进提示词 | 每个项目重搭一遍 RAG |
| **生产行为** | 重试分类、限流器、熔断器，以及按次运行的 token/工具/耗时指标 | 先祈祷，再复盘 |
| **一张脸** | GPU 渲染的原生窗口（GPUI），不是 Electron 壳 | 给 CLI 套个网页 |

---

## 运行时内部（BOS）

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

## BSH：原生 AI Agent 工作空间

**BSH 是构建、使用和扩展 BOS 智能体的桌面应用。**
它基于 GPUI 构建，不是 Electron 套壳；把 Agent 执行过程与开发者日常使用的工具放进同一个工作空间。

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

BOS 面向嵌入式集成，BSH 面向可扩展桌面工作流。BSH 扩展以 `extension.toml` 描述。
宿主负责校验 Manifest、验证包内容、解析依赖、管理生命周期，并在扩展请求宿主能力时执行授权策略。

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
