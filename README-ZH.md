<p align="right">
  <a href="README.md">English</a> · <b>中文</b>
</p>

<p align="center">
  <img src="docs/assets/bos-logo.png" alt="BOS — BrainOS" width="440">
</p>

<p align="center">
  <strong>用 Rust 写的智能体运行时，以及建立在它之上的桌面应用。</strong>
</p>

<p align="center">
  <b>BOS</b>（BrainOS）是运行时：智能体、工具、技能、记忆、MCP 与多智能体总线，<br>
  同一套 Rust 内核，同时提供 Python 与 JavaScript 绑定。<br>
  <b>BSH</b>（BOS Harness）是构建在同一运行时之上的原生桌面应用：工作区、对话、终端、<br>
  浏览器、编辑器，以及能触及以上每一个界面的扩展平台。
</p>

<p align="center">
  <a href="https://pypi.org/project/nbos"><img src="https://img.shields.io/pypi/v/nbos?label=python&logo=pypi&color=3776AB" alt="PyPI"></a>
  <a href="https://www.npmjs.com/package/@open1s/jsbos"><img src="https://img.shields.io/npm/v/@open1s/jsbos?label=javascript&logo=npm&color=CB3837" alt="npm"></a>
  <a href="https://github.com/open1s/bos/actions/workflows/rust.yml"><img src="https://github.com/open1s/bos/actions/workflows/rust.yml/badge.svg" alt="Rust CI"></a>
  <a href="https://github.com/open1s/bos/actions/workflows/binding-parity.yml"><img src="https://github.com/open1s/bos/actions/workflows/binding-parity.yml/badge.svg" alt="Binding parity"></a>
  <a href="https://github.com/open1s/bos/blob/main/LICENSE"><img src="https://img.shields.io/github/license/open1s/bos?color=blue" alt="License"></a>
  <a href="docs/BOS-BSH-Textbook.md"><img src="https://img.shields.io/badge/docs-34%20chapters%20%C2%B7%20second%20edition-blue" alt="Textbook"></a>
</p>

---

## 一个仓库，两件东西

| | 它是什么 | 从这里开始 |
|---|---|---|
| **BOS**<br>运行时 | 一套 Rust 内核（`agent`、`react`、`bus`、`config`、`logging`），加上 Python（`nbos`）与 JavaScript（`@open1s/jsbos`）绑定。想把智能体嵌进**你自己的程序**，用这个。 | `pip install nbos`<br>`npm install @open1s/jsbos`<br>`agent = { path = "crates/agent" }` |
| **BSH**<br>桌面端 | 原生桌面应用。想**直接使用**智能体——会话、工作区、终端、浏览器、编辑器、子智能体、定时任务、扩展——打开这个。 | `cargo build -p bsh` |

BSH 不是另起炉灶的第二个产品，而是 BOS 的第一个使用者：同一套智能体循环、同一批工具、
同一个沙箱、同一个 MCP 客户端。BSH 获得的能力，crates 也会有；两种绑定随后跟上。

---

## 30 秒上手

**Python——带工具的智能体：**

```bash
pip install nbos
```

```python
from nbos import BrainOS, tool

@tool("两数相加")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").with_tools(add)
    print(await agent.ask("2+2 等于多少？"))
```

**JavaScript——同一个智能体，同一套词汇：**

```bash
npm install @open1s/jsbos
```

```javascript
import { BrainOS, ToolDef } from '@open1s/jsbos';

const add = new ToolDef(
  'add', '两数相加',
  (args) => (args.a || 0) + (args.b || 0),
  { type: 'object', properties: { result: { type: 'number' } }, required: ['result'] },
  { type: 'object', properties: { a: { type: 'number' }, b: { type: 'number' } }, required: ['a', 'b'] },
);

const brain = new BrainOS();
await brain.start();
const agent = await brain.agent('assistant').register(add).start();
console.log(await agent.runSimple('2+2 等于多少？'));
```

**Rust——两种绑定共用的内核：**

```rust
use agent::{Agent, AgentConfig};

#[tokio::main]
async fn main() {
    let config = AgentConfig::default()
        .name("assistant")
        .model("openai/gpt-4o")          // 厂商由模型前缀推断
        .api_key(std::env::var("OPENAI_API_KEY").unwrap_or_default());
    let agent = Agent::from_config(config);
    println!("{}", agent.run_simple("Hello").await.expect("agent run"));
}
```

**桌面应用——不是 REPL，是一扇窗：**

```bash
git clone https://github.com/open1s/bos && cd bos
cargo build --release -p bsh
./target/release/bsh                  # 在 设置 → 模型 里选一个模型
./crates/bsh/make-dev-app.sh          # macOS：同一个二进制，包成 BSH.app
```

BSH 需要 Rust 1.97+ 与 **Zig 0.16**（终端面板要编译 Ghostty）。在 Linux 上，
[`.github/workflows/rust.yml`](.github/workflows/rust.yml) 里 CI 安装的那串 GTK、WebKitGTK、
Wayland 与 X11 依赖，就是构建真正需要的全部。`make-dev-app.sh` 构建的是 debug 二进制，
包装成 `target/debug/BSH.app`。

### 配置模型

**BOS** 读取 `~/.bos/conf/config.toml`：

```toml
[global_model]
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"
model = "gpt-4"

# ……或者按厂商分节，由模型前缀选择
[llm.nvidia]
api_key = "nv-..."
base_url = "https://integrate.api.nvidia.com/v1"
model = "nvidia/llama-3.1-nemotron-70b-instruct"

[llm.google]
api_key = "google-api-key"
base_url = "https://generativelanguage.googleapis.com/v1"
model = "gemini-pro"

[llm.openrouter]
api_key = "or-..."
base_url = "https://openrouter.ai/api/v1"
model = "anthropic/claude-3-haiku"
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
| **工具真的能跑** | 唯一一套沙箱实现（macOS SBPL、Linux Landlock + Seccomp、Windows AppContainer），同时守住工具与终端两条缝 | “模型都这么说了，应该没事” |
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

tools = await client.list_tools()
print(await client.call_tool("echo", '{"text": "hello"}'))
```

JavaScript 客户端是同样的调用，只是换成 camelCase（`listTools`、`callTool`、`connectHttp`）；
Rust 宿主拿到的是同一个客户端。

### 两种绑定永远同步

`nbos` 与 `@open1s/jsbos` 不是互相翻译的两份实现，而是同一引擎上的两张皮，被同一套词汇
（`ask`、`call`、`publish_text`、`recv`、`handle` ……）约束；每个 PR 上都有三道静态校验：
[`parity.test.js`](crates/jsbos/test/parity.test.js) 对比 Python 与 JavaScript 的类，
[`native-bindings.test.js`](crates/jsbos/test/native-bindings.test.js) 证明每个包装调用都落到真实的
原生成员，[`api-docs.test.js`](crates/jsbos/test/api-docs.test.js) 证明每个公开成员都有文档。

| | Python | JavaScript |
|---|---|---|
| 导入 | `from nbos import BrainOS, tool` | `import { BrainOS, ToolDef } from '@open1s/jsbos'` |
| 创建运行时 | `async with BrainOS() as brain:` | `const brain = new BrainOS(); await brain.start()` |
| 创建智能体 | `brain.agent("name")` | `brain.agent("name")` |
| 选模型 | `.with_model("gpt-4")` | `.model("gpt-4")` |
| 注册工具 | `.with_tools(add)` | `.register(toolDef)` |
| 运行 | `await agent.ask("…")` | `await agent.runSimple("…")` |
| 多模态 | `Content.parts([...])` | `Content.parts([...])` |
| 底层接口 | `from nbos import Agent, Bus, McpClient` | `import { Agent, Bus, McpClient } from '@open1s/jsbos'` |

---

## 桌面端内部（BSH）

`bsh` 是一个原生二进制。下面每一样都在里面，都由本仓库的 crates 支撑——没有一个是摆设：

- **工作区与会话**——一个工作区就是一个目录、一条边界；会话会被保存、搜索、置顶、归档、
  fork，并在启动时恢复。
- **对话**——流式轮次、可跳转的轮次栏、由智能体自己的 plan store 驱动的待办面板、转轮中
  输入的消息队列、边流边列的子智能体，以及按节奏打进会话的定时任务。
- **真正的终端**——Ghostty 的 Metal 画面嵌在窗口里；每一条提交的命令行都先过终端策略，
  再到 shell。
- **浏览器面板**——用 WebView（macOS 上是 WKWebView，Linux 上是 WebKitGTK）打开网页，
  `bsh --browser=URL` 直接落到某一页。
- **编辑器**——文件树、标签页、diff，以及人们真会打开的那些语言的 tree-sitter 语法。
- **有意义的设置**——账户、保险库、通用、模型、插件、智能体预设；一份每次启动只读一次的
  `settings.toml`，以及一个用 Argon2id + XChaCha20-Poly1305 封起来的加密保险库，存放不该
  明文躺在设置里的值。
- **两种语言**——中文与英文，逐个面板翻译，而不是只翻译菜单。
- **扩展**——见下。一个包可以加一个工具、一条命令、一个面板、一段提示词、一个上下文、
  一项技能、一个主题、一个图标、一项语言服务、一个智能体角色、一条工作流、存储、事件，
  或一个观察 hook。

---

## 扩展：一份 manifest，六种运行时

一个包就是一个目录：一个 `extension.toml`，加上它声明的贡献。宿主会校验它、算哈希、
装进内容寻址的存储、解析依赖、跑生命周期，然后在每次调用时把请求交给能力策略——按调用、
带作用域地决定这个包能不能做它想做的事。

<p align="center">
  <img src="docs/assets/charts/architecture.png" alt="从包到贡献：校验、哈希、安装、解析依赖、策略、分发、运行" width="820">
</p>

| 运行时 | 什么时候用 | 客体的形态 |
|---|---|---|
| **declarative** | 工作就是 JSON：工具、面板、提示词、主题 | 没有代码 |
| **process** | 工作已经是一个程序或 CLI | 子进程，逐行 JSON-RPC |
| **wasm-component** | 是新的第三方代码，需要硬隔离 | 对 WIT 契约的 WASM component |
| **extism** | 你已经有 Extism 插件 | 插件，带 `bsh_host` 导入面 |
| **mcp** | 你已经有 MCP 服务器 | 那台服务器，stdio |
| **built-in** | 你是第一方，且链接进宿主 | 本仓库里的 Rust |

<p align="center">
  <img src="docs/assets/charts/call-path.png" alt="每次工具调用：逐个授权声明的能力，带理由与审计行地拒绝，或带截止时间地调用" width="700">
</p>

```bash
# 先构建应用，再用它的开发者 CLI
export BSH=./target/release/bsh

$BSH ext new com.acme.hello --dir hello   # 生成包骨架
$BSH ext validate hello                   # 模式、id、文件、兼容性
$BSH ext mock hello echo '{"text":"hi"}'  # 在宽松的 mock 宿主上跑一个工具
$BSH ext install hello                    # 暂存、哈希、授权、激活
$BSH ext list                             # 宿主认为已经装了什么
$BSH ext doctor                           # 哈希漂移、依赖与策略缺口
$BSH ext grant com.acme.hello workspace.write --scope workspace
$BSH ext logs com.acme.hello              # 逐条决策的审计轨迹
```

从能跑的包开始读：[`examples/extensions/hello`](examples/extensions/hello) 是声明式的，
[`process-echo`](examples/extensions/process-echo) 讲 JSON-RPC，
[`wasm-echo`](examples/extensions/wasm-echo) 与 [`extism-echo`](examples/extensions/extism-echo) 是沙箱里的，
[`mcp-echo`](examples/extensions/mcp-echo) 复用你已有的服务器，[`extism-hook`](examples/extensions/extism-hook)
观察模型与 HTTP 流量。每次推送，CI 都会验证全部示例
（[`scripts/ci/verify-bsh-examples.sh`](scripts/ci/verify-bsh-examples.sh)）。

- **规范契约：**[`docs/bsh-extension-standard.md`](docs/bsh-extension-standard.md) · **上手教程：**
  [`docs/bsh-extension-dev-guide.md`](docs/bsh-extension-dev-guide.md) · **WIT：**
  [`crates/bsh/wit/extension.wit`](crates/bsh/wit/extension.wit)
- **二十项能力**，从 `workspace.write`、`network.http`，到 `secrets.read`、`process.execute` 与
  `hooks.observe`；每一项都能带作用域授权，每次决策都会写进审计日志。
- **hook 不是 event：**事件是事后发生、可以有很多订阅者；hook 是声明过的缝调用、只有一类
  接收者。见 [`docs/bsh-hook-system.md`](docs/bsh-hook-system.md)。
- **签名与信任：**`bsh ext keygen`/`sign`/`trust`/`verify`，依赖 lockfile，以及一个可以指向
  任意 Git 仓库的市场。

---

## 仓库地图

```
crates/
├── agent/        智能体、工具、技能、记忆、MCP、会话、沙箱、策略
├── react/        ReAct 引擎与 LLM 客户端缝
├── bus/          发布/订阅、查询、caller/callable
├── config/       TOML/YAML/环境变量加载与发现
├── logging/      tracing 与埋点
├── qserde/       总线使用的 rkyv 线格式
├── resource/     独立资源子系统与 `rex` CLI
├── bsh/          BSH 桌面端（二进制 + 扩展平台）
├── nbos/         Python 绑定 -> pip install nbos
└── jsbos/        Node.js 绑定 -> npm install @open1s/jsbos
```

分层是单向的：`nbos`/`jsbos` → `agent` → `react` → `bus` → `logging` → `config`；
`resource` 自成一系。长版本见 [ARCHITECTURE.md](ARCHITECTURE.md)。

---

## 示例

| | |
|---|---|
| **Python** | [`crates/examples/`](crates/examples)——智能体、总线、MCP（stdio 与 HTTP）、hook、插件；[`crates/nbos/examples/`](crates/nbos/examples) 是多模态内容与音频 |
| **JavaScript** | [`crates/jsbos/examples/`](crates/jsbos/examples)——同一套，另有 MCP over HTTP 的智能体 |
| **Rust** | [`crates/agent/examples/`](crates/agent/examples)——快速上手、hook 与插件、韧性；[`crates/react/examples/`](crates/react/examples) 是裸 LLM 调用 |
| **扩展** | [`examples/extensions/`](examples/extensions)——每种运行时一个包，全部由 CI 验证 |

---

## 文档

| 从这里开始 | |
|---|---|
| [**BSH by Example**](docs/BOS-BSH-Textbook.md) | 34 章实战书，讲桌面端与它的扩展框架——第二版 |
| [Python 指南](docs/python-user-guide.md) · [JavaScript 指南](docs/javascript-user-guide.md) · [Rust 指南](docs/rust-user-guide.md) | 三个接口面，都有完整示例 |
| [扩展标准](docs/bsh-extension-standard.md) · [开发指南](docs/bsh-extension-dev-guide.md) | 规范契约，以及做出一个包的最短路径 |
| [架构](ARCHITECTURE.md) · [上手](ONBOARDING.md) | 分层怎么咬合，以及怎样在这个仓库里开始干活 |
| [设置规范](docs/bsh-settings-spec.md) · [终端策略](docs/bsh-terminal-policy.md) · [Hook](docs/bsh-hook-system.md) · [观测捕获](docs/bsh-observability-capture.md) | 某个面板背后的真实行为 |
| [`rex` CLI](docs/rex-user-guide.md) · [诊断](docs/diagnostics.md) | 独立的资源工具，以及出问题时的排查 |
| [Wiki](https://github.com/open1s/bos/wiki) · [更新日志](CHANGELOG.md) | 指南与发布说明 |

---

## 构建、测试、贡献

```bash
cargo build --workspace                  # 全部
cargo test --workspace                   # 核心工作区（绑定有自己的测试套件）
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

# 绑定
cd crates/nbos  && maturin develop && pytest -m "not llm"
cd crates/jsbos && npm install && npx ava
```

- **CI** 会跑工作区测试、clippy、rustdoc、扩展示例验证器，以及两条绑定流水线：
  [`.github/workflows/`](.github/workflows/)
- **参与：**[CONTRIBUTING.md](CONTRIBUTING.md) · **安全：**[SECURITY.md](SECURITY.md) ·
  **版本管理：**本仓库使用 [jj](https://github.com/jj-vcs/jj)——每个改动都是一摞你要在推送前
  自己整理干净的历史。
- **适合第一次贡献的：**为你天天用的某个工具写一个扩展、加一门语法或语言服务，或者认领一个
  标了 `good first issue` 的 issue。

如果 BSH 或 BOS 替我省下了一天，**给仓库点个 star**——它是让这件事继续被投入的信号，
也是下一个人找到它的方式。

---

## 许可证

MIT OR Apache-2.0 · **版本** 3.0.2 · [发布记录](https://github.com/open1s/bos/releases)
