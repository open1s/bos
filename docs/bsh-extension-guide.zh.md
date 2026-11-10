# BSH 扩展开发指南

**教材版 · 基于 BSH 3.0.2 / Ferris 框架 / manifest 2.0**

这是一本用来"学会写 BSH 扩展"的书，不是规范。规范是
[bsh-extension.md](bsh-extension.md)：当本书与规范或代码冲突时，以规范和代码为准，
本书就算有 bug。英文案例书 [BOS-BSH-Textbook.md](BOS-BSH-Textbook.md) 覆盖同一套
平台，偏"逐项查证"；本书偏"课程"：每一章都有学习目标、可运行实验、常见错误和练习。

全书按一条主线组织：

> 一个包（package）→ 一份清单（manifest）→ 一个运行时（runtime）→ 一组贡献
> （contribution）→ 宿主表面（tool / command / panel / prompt / event / hook）。

你不需要先读完全书再动手。第 1 章就能装出一个能用的扩展；第 2 章解释你刚才
做了什么；第 3 章之后再把每一层拆开。

---

## 这本书写给谁

| 你的背景 | 建议路径 |
|---|---|
| 没写过 BSH 扩展，想先跑通 | 第 1、2、3、5、7 章 |
| 要做一个给 agent 用的工具包 | 第 3、5、6、12、14、17 章 |
| 要做可视化面板或工作台 | 第 7、9、14、28 章 |
| 要接外部服务 / MCP / Node 生态 | 第 10、13、14、16、17 章 |
| 要做 LLM / 流量观测 | 第 22、23、24、25 章 |
| 要给团队上课 | 第 34–37 章（课程大纲、实验、习题、量规） |
| 要从 1.x 老包迁移 | 第 3、33 章 |

## 版本与基线

本书描述的是下面这组基线，写作时逐条对照过源码：

| 项 | 值 |
|---|---|
| 宿主版本 | BSH 3.0.2 |
| extension API | 1.0.0 |
| manifest schema | 2.0（唯一接受的版本） |
| 框架分层 | `ferris-core` / `ferris-runtime` / `ferris-sdk` / `crates/bsh` |
| 能力词表 | 21 项（含 `hooks.observe`、`agent.intercept`） |
| 已交付钩子点 | 5 个，仅 observe 类，仅 Extism 运行时 |
| 运行时 | builtin / declarative / process / mcp / extism / wasm-component / cordis |

> **读法提醒**：本书所有命令输出都来自 3.0.2 的 `target/debug/bsh`。附录 I
> 记录了写作时发现的、规范与代码不一致的地方（例如 `bsh ext sign` 仍用 1.x
> 解析器读清单）。读到相关章节时先看附录 I，能省掉一次误判。

## 相关文档与读法

| 文档 | 用途 | 状态 |
|---|---|---|
| [bsh-extension.md](bsh-extension.md) | 规范：清单、能力、贡献、生命周期 | 规范性，冲突时以它和源码为准 |
| [ferris-framework.md](ferris-framework.md) | 分层、阶段、冻结决策 | 设计说明 |
| [ferris-sdk.md](ferris-sdk.md) | Rust 作者 API 与宿主门面 | 作者指南 |
| [bsh-hook-system.md](bsh-hook-system.md) | 钩子层与五个点 | 部分为设计态（intercept 等未实现） |
| [bsh-observability-capture.md](bsh-observability-capture.md) | LLM/流量采集数据模型 | 部分为设计态（正文、帧统计未实现） |
| [bsh-tool-policy.md](bsh-tool-policy.md) | 工具策略与审批 | 规范 |
| [bsh-settings-spec.md](bsh-settings-spec.md) | 设置项 | 规范 |
| [bsh-subagents.md](bsh-subagents.md) | 子代理 | 设计说明 |
| [bsh-terminal-policy.md](bsh-terminal-policy.md) | 终端策略 | 规范 |
| [BOS-BSH-Textbook.md](BOS-BSH-Textbook.md) | 英文案例书（逐项查证） | 与本书互补 |
| [examples/extensions](../examples/extensions) | 可运行示例 | 代码 |

阅读建议：先读本书建立主线，再按需查规范原文；设计文档里"计划/目标态"的
描述不要当成已交付能力，附录 I 列出了写作时确认的偏差。

## 约定

- 命令都在仓库根目录执行，先构建一次：

  ~~~bash
  cargo build -p bsh
  export BSH=./target/debug/bsh
  ~~~

- `$BSH` 指这个二进制。`--root <dir>` 把实验隔离在指定目录；不加 `--root`
  时，扩展根是 `~/.bos/bsh/extensions`（`BSH_EXTENSION_ROOT` 可覆盖）。
- "退出码 1"表示被拒绝（不是崩溃）；"退出码 2"是用法错误。
- 输出里的路径按你实际的工作目录变化；`hash`、时间戳、pid 每次都不同。
- 本书把"宿主"（host，即 BSH）与"扩展"（extension）分开说；"模型"指 LLM；
  "读者"指坐在 BSH 前面的人。

## 目录

**第一部分 入门**

- [第 1 章 十分钟建立第一个扩展](#ch1)
- [第 2 章 心智模型](#ch2)
- [第 3 章 包结构与 manifest 2.0](#ch3)
- [第 4 章 贡献、命名与宿主表面](#ch4)

**第二部分 声明式能力**

- [第 5 章 工具](#ch5)
- [第 6 章 命令、提示、上下文与技能](#ch6)
- [第 7 章 面板与声明式 UI](#ch7)
- [第 8 章 事件](#ch8)
- [第 9 章 配置、存储与描述性贡献](#ch9)

**第三部分 运行时**

- [第 10 章 运行时总览与选择](#ch10)
- [第 11 章 declarative 运行时](#ch11)
- [第 12 章 process 运行时](#ch12)
- [第 13 章 MCP 运行时](#ch13)
- [第 14 章 Extism 运行时](#ch14)
- [第 15 章 WASM component 运行时](#ch15)
- [第 16 章 Cordis 运行时](#ch16)

**第四部分 安全、生命周期与分发**

- [第 17 章 能力与授权](#ch17)
- [第 18 章 最小权限与拒绝诊断](#ch18)
- [第 19 章 生命周期、状态与激活](#ch19)
- [第 20 章 存储、哈希、签名与分发](#ch20)
- [第 21 章 依赖与服务](#ch21)

**第五部分 观测**

- [第 22 章 观测钩子：原理与契约](#ch22)
- [第 23 章 声明、授权并实现一个观察器](#ch23)
- [第 24 章 采集与分析食谱](#ch24)
- [第 25 章 钩子调试](#ch25)

**第六部分 工程化**

- [第 26 章 测试策略](#ch26)
- [第 27 章 调试手册](#ch27)
- [第 28 章 CLI 任务手册](#ch28)
- [第 29 章 打包、签名、发布与市场](#ch29)
- [第 30 章 内建扩展与第一方扩展](#ch30)

**第七部分 Rust SDK 与架构**

- [第 31 章 用 ferris-sdk 编写 Rust 扩展](#ch31)
- [第 32 章 平台架构与贡献租约](#ch32)
- [第 33 章 从 manifest 1.x 迁移](#ch33)

**第八部分 教学材料**

- [第 34 章 12 周课程大纲](#ch34)
- [第 35 章 实验手册](#ch35)
- [第 36 章 习题与参考答案要点](#ch36)
- [第 37 章 结课项目与评分量规](#ch37)

**附录**

- [A. manifest 2.0 速查](#appA)
- [B. 能力矩阵](#appB)
- [C. 钩子载荷参考](#appC)
- [D. 错误目录](#appD)
- [E. 面板节点参考](#appE)
- [F. 示例索引](#appF)
- [G. 审查清单](#appG)
- [H. 术语表](#appH)
- [I. 3.0.2 现状核对与已知偏差](#appI)

---

<a id="ch1"></a>
# 第 1 章 十分钟建立第一个扩展

**学习目标**：安装一个包，看懂宿主对它的判断，在不启动 agent 的情况下运行一个工具，
然后把它装进一个隔离的实验仓库。

这一章不解释所有字段。先把"包能跑"这件事变成肌肉记忆。

## 1.1 构建并生成脚手架

~~~bash
cargo build -p bsh
export BSH=./target/debug/bsh
$BSH ext new com.example.hello --dir /tmp/hello-lab
~~~

输出：

~~~text
created /tmp/hello-lab for com.example.hello
next: bsh ext validate /tmp/hello-lab
~~~

脚手架只生成四个文件，但已经是一个完整包：

~~~text
hello-lab/
  extension.toml
  tools/echo.json
  commands/refresh.md
  README.md
~~~

`extension.toml` 长这样（`bsh ext new` 的原样输出）：

~~~toml
schema_version = "2.0"

[extension]
id = "com.example.hello"
name = "Hello"
version = "0.1.0"
description = "A BSH extension"

[compatibility]
host = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[capabilities]
required = ["agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"

[[contribution]]
kind = "command"
id = "refresh"

[resources]
max_execution_ms = 5000
~~~

注意两件事：

1. **能力是"声明"，不是"授权"**。清单里写了 `agent.tools.register`，宿主仍会
   按信任级别与策略决定是否放行（第 17 章）。
2. **贡献是通用信封**。`[[contribution]]` 只有 `kind`、`id`、`payload` 三个
   字段；具体文件由 kind 的宿主适配器解释（第 3、4 章）。

## 1.2 四种"检查"，从轻到重

初学者最容易混淆 `validate`、`test`、`mock`、`install`。它们的分工是：

| 命令 | 读清单 | 读贡献文件 | 运行代码 | 写托管状态 |
|---|---:|---:|---:|---:|
| `validate` | 是 | 是（校验声明与文件一致性） | 否 | 否 |
| `test` | 是 | 是 | 否（只做运行时预检） | 否 |
| `mock` | 是 | 是 | 是（宽松 mock 宿主） | 否（临时目录） |
| `install` | 是 | 是 | 激活时可能启动 | 是 |

先跑 `validate`：

~~~bash
$BSH ext validate /tmp/hello-lab
~~~

~~~text
id:      com.example.hello
version: 0.1.0
runtime: declarative
schema:  2.0
contributions: 2
hash:    60e15cf50f7cf0be938f8f40c8cc27b32906a5f5f2baa8afe2f58397beefe9b3
valid
~~~

`hash` 是规范化后的包哈希：按相对路径排序、忽略 `.git`、`target`、
`node_modules`、`.jj`，不含 mtime 与权限位（第 20 章）。它会在安装、签名、
锁文件和 `doctor` 里反复出现。

再跑 `test`：

~~~bash
$BSH ext test /tmp/hello-lab
~~~

~~~text
valid com.example.hello
version: 0.1.0
runtime: declarative
tools: 1
commands: 1
panels: 0
workflows: 0
~~~

## 1.3 不启动 agent，运行一个工具

`mock` 会构造一个"什么能力都批"的临时宿主，把工具跑一次：

~~~bash
$BSH ext mock /tmp/hello-lab echo '{"text":"hi"}'
~~~

~~~json
{
  "text": "hi"
}
~~~

为什么输出不是 `{ "content": [...] }`？因为 `echo_arguments` 返回的是原始
JSON，宿主会把它规范化为工具结果；打印时保留最简形态（第 5 章）。

`mock` 的边界要记牢：

- 它**不看**真实授权。一个在 `mock` 里通过的工具，可能因为缺少能力在真实宿主
  里被拒绝。
- 它**不跑** agent、不投递钩子、不打开面板。
- 它用 `{"mock": true}` 作为配置；需要配置分支的代码要能接受这个值。

## 1.4 装进隔离仓库

真实 store 在 `~/.bos/bsh/extensions`。实验用 `--root` 隔离：

~~~bash
$BSH ext install /tmp/hello-lab --root /tmp/hello-root
~~~

~~~text
installed com.example.hello 0.1.0
source:  development
hash:    60e15cf50f7cf0be938f8f40c8cc27b32906a5f5f2baa8afe2f58397beefe9b3
path:    /tmp/hello-root/installed/com.example.hello/0.1.0
~~~

然后看宿主怎么描述它：

~~~bash
$BSH ext list --root /tmp/hello-root
$BSH ext info com.example.hello --root /tmp/hello-root
$BSH ext plan --root /tmp/hello-root
$BSH ext why com.example.hello --root /tmp/hello-root
$BSH ext effects com.example.hello --root /tmp/hello-root
~~~

你会看到（内置扩展会一并出现）：

~~~text
ID                                 VERSION    STATE        TRUST      APPLIED  TOOLS
com.bos.bsh.annotations            1.0.0      active       builtin    yes
com.bos.bsh.diagnostics            1.0.0      active       builtin    yes      diagnostics__graph, diagnostics__why
com.bos.bsh.extensions             1.0.0      active       builtin    yes      extensions__list
com.example.hello                  0.1.0      active       local      yes      hello__echo
~~~

`info` 会给出一个包的全部"身份证"：

~~~text
id:          com.example.hello
name:        Hello
version:     0.1.0
runtime:     declarative
trust:       local
source:      development
root:        /tmp/hello-root/installed/com.example.hello/0.1.0
hash:        60e15cf5...
signature:   unsigned
state:       active
health:      healthy
configured:  true
applied:     true
tools:       hello__echo
commands:    hello__refresh
capability:  agent.tools.register [low] allowed (any)
~~~

三个细节值得停一下：

- **工具名被"资格化"了**：声明是 `echo`，模型看到的是 `hello__echo`
  （扩展 id 的最后一段 + `__` + 贡献 id）。第 4 章讲规则。
- **`capability` 行是当前决策，不是清单回显**：低风险能力对 local 信任默认放行。
- **`effects` 是"这个包会碰什么"的静态视图**，写作 `tool:echo`、`command:refresh`。

## 1.5 一条命令看清"为什么没跑起来"

~~~bash
$BSH ext why com.example.hello --root /tmp/hello-root
~~~

~~~text
com.example.hello: applied
~~~

如果它没被应用，`why` 会直接说出原因。例如安装一个声明了 `hooks.observe`
却没有授权的包：

~~~bash
$BSH ext install examples/extensions/extism-hook --root /tmp/hello-root
$BSH ext why com.example.extism-hook --root /tmp/hello-root
~~~

~~~text
com.example.extism-hook: not applied: extension com.example.extism-hook requires
capability hooks.observe, which policy denied: hooks.observe requires an explicit
grant for a local extension (run 'bsh ext grant com.example.extism-hook hooks.observe')
~~~

这句话同时给出了**原因**和**下一步命令**。第 17–18 章会把这条链路拆开。

## 1.6 本章实验

1. 把 `echo` 工具的描述改一句话，重新 `validate`，观察 `hash` 变化。
2. 删掉 `tools/echo.json`，再 `validate`。记下错误信息（第 27 章会用到）。
3. 在 `commands/refresh.md` 的 frontmatter 里加 `description: 刷新`，再 `test`。
4. 用 `--root /tmp/hello-root` 安装、`disable`、`enable`、`uninstall`，
   每一步都跑一次 `list`，观察 `STATE` 与 `APPLIED` 两列。

## 1.7 常见错误（现在就记住）

| 现象 | 原因 | 处理 |
|---|---|---|
| `unsupported manifest schema "1.0"` | 包是 1.x | `bsh ext migrate <path> --write` |
| `unknown field ...` | 2.0 解析器拒绝未知字段 | 对照第 3 章与附录 A |
| `declared contribution file is missing` | 清单声明了文件但没提供 | 补文件或删声明 |
| `... is not a declared tool` | 语言/集成引用了未声明工具 | 先声明工具 |
| `mock` 通过、真实调用被拒 | mock 不检查授权 | 看 `why` 与 audit |
| 装了但 `APPLIED no` | 缺能力、依赖或显式启用 | `why` / `plan` / `doctor` |

---

<a id="ch2"></a>
# 第 2 章 心智模型

**学习目标**：能画出"包 → 清单 → 运行时 → 贡献 → 表面"的链路；能解释一次工具
调用经过哪些层；能区分事件与钩子。

## 2.1 三个平面

BSH 的扩展平台分三层，每层只依赖右边：

~~~text
crates/bsh            宿主适配：agent 接线、UI、CLI、设置
   ↑
ferris-sdk            作者 API + 宿主门面（BSH 唯一的 Ferris 入口）
   ↑
ferris-runtime        标准运行时：declarative/process/mcp/extism/wasm/cordis
   ↑
ferris-core           清单、包存储、管理器、能力策略、服务、事件、UI 契约、签名
~~~

BSH 的 `Cargo.toml` 里只有一条 Ferris 依赖：

~~~toml
ferris-sdk = { path = "../ferris-sdk", features = ["host-runtime"] }
~~~

BSH 源码不直接 `use ferris_core` 或 `use ferris_runtime`，而是通过
`ferris_sdk::*` 与 `ferris_sdk::host_runtime::*`。这条边界由
`scripts/ci/check-ferris-independence.sh` 守着（第 32 章）。

对写扩展的人，这条分层只意味着两件事：

- 你写的"包"是**数据 + 可选代码**，由运行时解释；
- 你看到的错误码、能力名、贡献 kind 都是**宿主注册的词表**，不是 Rust 枚举的
  内部细节。

## 2.2 一个包怎样变成一个能力

~~~text
包目录
  extension.toml ──解析──▶ ManifestV2 ──规范化──▶ 内部 Manifest
       │                                        │
       │                                        ├─ 依赖 / 服务解析
       │                                        ├─ 兼容性检查
       │                                        └─ 能力声明
       ▼
  contribution 列表 ──按 kind 加载──▶ 贡献对象（工具描述、命令模板、面板文档…）
       │
       ▼
  Runtime（按 runtime.kind 构建）──tools()/call_tool()/panel()/command()/hook()
       │
       ▼
  宿主表面：agent 工具、命令面板、面板、提示、事件订阅、钩子目标
~~~

**关键区分**：

- **清单是契约**。清单说"我会提供 tool:echo"，宿主就要求
  `tools/echo.json` 存在；缺文件会让整个包 invalid，而不是"少提供一个工具"。
- **运行时是执行者**。同一个 tool 贡献，在 declarative 包里是纯函数，在 process
  包里是一次 JSON-RPC，在 Extism 包里是一次 wasm 调用。
- **贡献是宿主表面的一行**。谁消费它，决定了它在 UI 里长什么样。

## 2.3 一次工具调用的完整路径

以模型调用 `hello__echo` 为例：

~~~text
模型输出 tool_call
   │
   ▼
agent 工具注册表按资格名找到 ExtensionTool
   │  1. 构造 CallScope（从参数里的 path/url/secret 提取）
   │  2. 对描述符里的每个能力向 PolicyEngine 询问
   │  3. 每次决策写一行 audit
   ▼
runtime.call_tool("echo", args, ctx)
   │  declarative: implementation.run()
   │  process:     JSON-RPC tools/call
   │  extism:      bsh_call_tool
   │  ...
   ▼
ToolResult（content / structuredContent / isError）
   │
   ▼
写回 agent transcript；发 tool.completed 事件（若订阅）
~~~

三个"失败点"要分清：

1. **注册失败**：包没应用，工具根本不在注册表（`why` 能解释）。
2. **授权失败**：工具在，但能力不足；模型看到的是工具错误，不是崩溃。
3. **执行失败**：运行时返回错误（进程退出、wasm trap、MCP EOF），可能连带
   关闭会话（第 12–16 章）。

## 2.4 运行时的七个位置

| 运行时 | 本质 | 有代码吗 | 适合 |
|---|---|---:|---|
| `builtin` | 链进 BSH 的 Rust，按 id 注册 | 是（在宿主里） | 第一方能力、需要宿主内部接缝 |
| `declarative` | 包目录里的文件 | 否 | 纯数据工具、命令、提示、面板、描述符 |
| `process` | 外部进程，JSON-RPC over stdio | 是（任意语言） | 需要依赖、语言服务、复杂 IO |
| `mcp` | MCP server over stdio | 是 | 复用现成 MCP 生态 |
| `extism` | Extism PDK 插件（core wasm） | 是 | 沙箱内工具、观测钩子 |
| `wasm-component` | WebAssembly Component | 是 | 强契约、可移植的 guest |
| `cordis` | `apply(ctx, config)` 的 JS 包 | 是 | 复用 Node/DSH 生态 |

选择方法见第 10 章；每种运行时的完整契约见第 11–16 章。

## 2.5 事件不是钩子

这是全书最重要的一组概念，先立住：

| | 事件（event） | 钩子（hook） |
|---|---|---|
| 时机 | 事情发生**之后** | 发生在**接缝上**，数据还在手里 |
| 消费者 | 任意订阅者 | 声明了该点的包 |
| 投递 | 有界队列，满了丢弃并计数 | 有界队列，批处理；observe 不阻塞 |
| 能否改变结果 | 否 | observe 否；intercept 设计上可以，当前未实现 |
| 典型用途 | 通知、审计、UI 刷新 | 采集、指标、redaction 审查 |

事件在 agent 之外也能用；钩子只在宿主接缝上产生。第 8 章讲事件，第 22–25 章
讲钩子。

## 2.6 本章练习

1. 用一句话解释：为什么"清单里声明了工具"不等于"模型能调用它"？
2. 画出 `process` 包的一次工具调用，标出至少四个失败点。
3. 判断：`tool.completed` 事件能否阻止一次工具调用？钩子呢？

---

<a id="ch3"></a>
# 第 3 章 包结构与 manifest 2.0

**学习目标**：能独立写出一份通过的 2.0 清单；知道每类贡献的文件约定；能读懂
清单解析错误。

## 3.1 最小树与文件约定

~~~text
my-extension/
  extension.toml          必需，schema 2.0
  package.sig             可选，Ed25519 签名
  tools/<id>.json         工具描述符
  commands/<id>.md        命令模板
  panels/<id>.json        面板文档
  prompts/<id>.md         提示片段
  contexts/<id>.md        上下文片段
  skills/<id>/SKILL.md    技能目录
  workflows/<id>.json     工作流载荷
  themes/<id>.json        主题载荷
  presets/<id>.json       预设载荷
  providers/models/<id>.json
  providers/agents/<id>.json
  languages/<id>.json
  integrations/<id>.json
  schemas/*.json          配置 schema（约定）
  asset/*                 面板可引用的图片
~~~

规则：

- **只读清单声明的文件**。没声明的文件不会加载；声明了但缺失的文件让包 invalid。
- **路径相对包根，不得逃逸**。绝对路径、`..`、符号链接都会被拒绝（安装时
  遇到符号链接会让整个安装失败）。
- **大小有上限**：清单、贡献文件、UI 文档都有上限（附录 A.3 列了数）。

## 3.2 一份完整清单

下面是 `examples/extensions/hello/extension.toml`（真实文件，略有删节）：

~~~toml
schema_version = "2.0"

[extension]
id = "com.example.hello"
name = "Hello"
version = "1.0.0"
description = "A minimal declarative extension: one echo tool, one command and one panel"
authors = ["BSH Example"]
license = "MIT"

[compatibility]
host = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[runtime.config]
startup = "on-demand"

[capabilities]
required = ["agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"

[contribution.payload]

[[contribution]]
kind = "command"
id = "greet"

[contribution.payload]

[[contribution]]
kind = "panel"
id = "status"

[contribution.payload]

[[contribution]]
kind = "prompt"
id = "hello"

[contribution.payload]

[resources]
max_execution_ms = 2000
max_output_bytes = 65536

[dependencies]

[services]
provides = []
requires = []
~~~

`[contribution.payload]` 可以是空表，也可以放该 kind 的附加字段。当前标准实现
**以文件为主**：工具的 `description`、`input_schema`、`capabilities` 从
`tools/<id>.json` 读，命令/提示/上下文从 Markdown 读，面板从
`panels/<id>.json` 读。payload 留给宿主注册的 kind 使用，不要指望它能覆盖文件。

## 3.3 顶层字段

| 字段 | 必需 | 说明 |
|---|---:|---|
| `schema_version` | 是 | 必须是 `"2.0"` |
| `extension` | 是 | 身份块 |
| `compatibility` | 是 | 宿主与 API 版本范围 |
| `runtime` | 是 | 运行时 kind 与不透明配置 |
| `capabilities` | 否 | **只放 required** |
| `contribution` | 否 | 通用贡献数组 |
| `configuration` | 否 | JSON Schema 路径与默认值 |
| `resources` | 否 | 内存、执行、并发、输出上限 |
| `dependencies` | 否 | 扩展 id → semver 要求 |
| `services` | 否 | 提供/依赖的服务契约 |

**未知顶层字段会被拒绝。** 例如把 `unknown = 1` 加到 `[resources]` 后面：

~~~text
error: .../extension.toml: invalid TOML: TOML parse error at line 30, column 1
   |
30 | unknown = 1
   | ^^^^^^^
unknown field 'unknown', expected one of 'max_memory_mb', 'max_execution_ms',
'max_concurrent_calls', 'max_output_bytes'
~~~

注意错误信息给出的是"当前表"的允许字段——它说的是 `[resources]` 而不是顶层，
因为 TOML 的 `unknown` 落在了 `[resources]` 表里。读错误时先看它抱怨的是哪张表。

## 3.4 身份

| 字段 | 必需 | 规则 |
|---|---:|---|
| `id` | 是 | 反向域名，至少两段；每段 1–63 字节，小写字母/数字/`-`/`_`，首字符字母或数字，末字符字母或数字 |
| `name` | 是 | 人类可读名 |
| `version` | 是 | 语义化版本 |
| `description` | 是 | 一段话 |
| `authors` | 否 | 展示字符串 |
| `license` | 否 | SPDX 表达式 |
| `repository` | 否 | 源码 URL |
| `homepage` | 否 | 项目 URL |

id 是稳定、全局唯一的主键：目录名、存储命名空间、能力授权、审计记录都靠它。
它同时决定模型可见的工具名前缀（第 4 章），所以**改 id 等于改 API**。

## 3.5 兼容性

~~~toml
[compatibility]
host = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
~~~

- `host` 是支持的 BSH 版本范围；
- `extension_api` 是扩展 API 代际；
- 两者都在激活前检查，超范围直接拒绝。

## 3.6 运行时与启动策略

`runtime.kind` 必须是宿主注册的字符串：

| kind | 载荷 | 标准适配器使用的配置键 |
|---|---|---|
| `builtin` | 宿主里按 id 注册的 Rust 运行时 | 无 |
| `declarative` | 包文件 | `startup` |
| `process` | JSON-RPC 子进程 | `entry`, `args`, `env`, `protocol`, `startup` |
| `mcp` | stdio 上的 MCP server | `entry`, `args`, `env`, `startup` |
| `extism` | Extism PDK 插件 | `entry`, `startup` |
| `wasm-component` | WebAssembly Component | `entry`, `startup` |
| `cordis` | `apply(ctx, config)` 包 | `entry`, `startup` |

`startup` 取值：`on-startup`、`on-demand`（默认）、`on-workspace-open`、
`manual`。`runtime.config` 对清单解析器不透明，由运行时适配器校验。

## 3.7 能力与贡献

~~~toml
[capabilities]
required = ["agent.tools.register"]
~~~

2.0 只接受 `required`。声明不等于授权（第 17 章）。最多条数受验证限制，
未知能力名直接报错，并提示跑 `bsh ext capabilities`。

贡献条目：

~~~toml
[[contribution]]
kind = "tool"
id = "echo"

[contribution.payload]
~~~

- `kind` 与 `id` 必填；同一扩展内 `(kind, id)` 不得重复；
- 贡献 id 用点分语法（如 `git.status`），文件名用完整 id；
- 未知 kind 在投影到宿主类型视图时被拒绝。

标准 kind 与文件约定：

| kind | 文件 / 含义 |
|---|---|
| `tool` | `tools/<id>.json` |
| `command` | `commands/<id>.md` |
| `panel` | `panels/<id>.json` |
| `prompt` | `prompts/<id>.md` |
| `context` | `contexts/<id>.md` |
| `skill` | `skills/<id>/SKILL.md` |
| `workflow` | `workflows/<id>.json` |
| `theme` | `themes/<id>.json` |
| `preset` | `presets/<id>.json` |
| `model-provider` | `providers/models/<id>.json` |
| `agent-provider` | `providers/agents/<id>.json` |
| `language` | `languages/<id>.json` |
| `integration` | `integrations/<id>.json` |
| `event.subscribe` | 订阅的主题 |
| `event.publish` | 发布的主题 |
| `hook` | observe 钩子点 |

## 3.8 资源、配置、依赖、服务

~~~toml
[resources]
max_memory_mb = 128
max_execution_ms = 5000
max_concurrent_calls = 4
max_output_bytes = 1048576

[configuration]
schema = "schemas/config.schema.json"

[configuration.defaults]
verbose = false

[dependencies]
"com.example.base" = "^1.0"

[services]
provides = []
requires = []
~~~

- 资源是**请求值**，受宿主上限约束；省略时是 256 MiB、30000 ms、4 并发与
  1 MiB 输出。
- 配置用包内 JSON Schema 校验；值可用 `bsh ext config <id> [set|schema]` 读写
  （注意附录 I 的现状偏差）。
- 依赖是 id → semver 要求；循环与不满足都在激活前拒绝。
- 服务见第 21 章。

## 3.9 实验：把错误逐个造出来

在 `/tmp/hello-lab` 的副本上依次尝试，记下错误字段：

1. 把 `schema_version` 改成 `"1.0"` → 应提示运行 `migrate`。
2. 删掉 `[compatibility].host` → 缺字段。
3. 把贡献 id 改成大写 `Echo` → 贡献 id 语法错误。
4. 重复一条 `[[contribution]] kind="tool" id="echo"` → duplicate。
5. 声明 `tool = "missing"` 但不给文件 → 缺文件。
6. 在 `[runtime]` 里加未知键 → 未知字段。
7. 把 `[[contribution]]` 写成 `[contribution]` → 类型不匹配（表 vs 数组）。

## 3.10 清单检查清单

- [ ] id 反向域名、全小写、末段是合法命名空间；
- [ ] 版本语义化，升级时同步改依赖要求；
- [ ] 每个 `[[contribution]]` 都有对应文件；
- [ ] 能力只写真正需要的，高风险能力有理由；
- [ ] `resources` 有超时与输出上限；
- [ ] 运行时的 `entry` 是包相对路径或白名单命令；
- [ ] `validate` 与 `test` 都过。

---

<a id="ch4"></a>
# 第 4 章 贡献、命名与宿主表面

**学习目标**：知道 16 种贡献分别进入哪个表面；能预测模型看到的工具名；能判断
一个贡献由宿主读取还是由运行时执行。

## 4.1 贡献不是插件 API

一个常见误解是"贡献 = 注册一个回调"。实际上贡献分两类：

| 类别 | 例子 | 谁消费 | 需要运行时吗 |
|---|---|---|---|
| **执行型** | tool、运行时可执行 command、动态 panel、hook | 运行时 | 是 |
| **内容型** | prompt、context、skill、command 模板、panel 初稿、theme、preset、provider、language、integration、workflow | 宿主/agent | 否（workflow 由包装器执行） |

内容型贡献即使在运行时暂时不可用的包里也能装、能显示；执行型贡献要等运行时可用。
这个区分能解释很多"装了但看不到"的问题。

## 4.2 资格名规则

工具名在 LLM API 里是**扁平全局命名空间**，所以宿主做一次资格化：

~~~text
extension id 的最后一段（sanitize：非字母数字变 _，转小写）
  + "__"
  + 贡献 id（点变下划线，sanitize）
~~~

例子：

| 扩展 id | 贡献 id | 模型可见名 |
|---|---|---|
| `com.example.hello` | `echo` | `hello__echo` |
| `com.example.git-assistant` | `git.status` | `git-assistant__git_status` |
| `com.bos.bsh.diagnostics` | `graph` | `diagnostics__graph` |

超过 64 字符时，保留前缀并追加 8 位十六进制摘要后缀，保证两个不同工具不会塌成
同一个名字。**命令与面板也用同样的资格名**（`hello__refresh`、
`hello__status`），但它们在 UI 里的显示名另取文件里的描述或标题。

## 4.3 十六种贡献一览

| kind | 表面 | 关键字段 | 典型能力 |
|---|---|---|---|
| `tool` | 模型可调用工具 | description、input_schema、capabilities、implementation | `agent.tools.register` |
| `command` | 命令面板模板；运行时可执行命令 | Markdown frontmatter + 正文 | `ui.commands` |
| `panel` | 宿主面板 | title、icon、location、default_visibility、view | `ui.panels` |
| `prompt` | 系统提示片段 | description + 正文 | `agent.prompts.provide` |
| `context` | 作用域上下文 | 同上 + `paths:` | `agent.context.provide` |
| `skill` | SKILL.md 技能 | 目录 + SKILL.md | `agent.skills.provide` |
| `workflow` | 包装成工具的步骤序列 | steps | `agent.workflows.provide` |
| `theme` | 面板配色 | 颜色 token | `ui.theme` |
| `preset` | agent/模块预设 | 预设文档 | `agent.agents.provide` |
| `model-provider` | 模型提供方 | provider 描述 | `agent.models.provide` |
| `agent-provider` | agent 角色 | role 描述 | `agent.agents.provide` |
| `language` | 语言服务 | name、extensions、tool、syntax | `agent.tools.register` |
| `integration` | 外部系统动作 | name、kind、action | `ui.commands` / 工具能力 |
| `event.subscribe` | 事件订阅 | 主题 | `events.subscribe` |
| `event.publish` | 事件发布 | 主题 | `events.publish` |
| `hook` | 观测钩子点 | 点名 | `hooks.observe` |

## 4.4 哪些贡献会进入 agent 构建

宿主只在包**已应用**（applied）且未挂起时安装它的 agent 贡献：

- 工具注册进工具表；
- prompt/context 以有界段落追加到系统提示（单段 8 KiB，总预算 64 KiB）；
- command 模板进入命令面板；
- panel 进入面板列表；
- skill 目录加入 agent 的技能搜索路径；
- hook 点加入观测目标。

注意 `prompt` 与 `context` 的区别：prompt 是**始终追加**的指令；context 可以
用 frontmatter 的 `paths:` 限定作用域（第 6 章）。

## 4.5 表面之间的调用关系

~~~text
                  ┌──────────────┐
   模型 ─────────▶│ tool         │──▶ runtime.call_tool
                  └──────────────┘
                  ┌──────────────┐
   读者 ─────────▶│ command 面板 │──▶ 插入模板（内容型）
                  └──────────────┘
                  ┌──────────────┐
   读者 ─────────▶│ panel        │──▶ action ──▶ tool（优先）
                  │              │            └▶ runtime.command（回退）
                  └──────────────┘
                  ┌──────────────┐
   集成动作 ─────▶│ integration  │──▶ action ──▶ tool 或 command
                  └──────────────┘
~~~

面板动作的语义值得单独记：**动作 id 先当工具找，找不到再当运行时命令**；两者
都受同一套每次调用能力检查（第 7 章）。

## 4.6 命名冲突

同一构建里，两个包声明的资格名相同会被拒绝：

~~~text
tool name <qualified> is also claimed by <other>
~~~

避免办法：id 的末段唯一（通常自然唯一）；不要用 `__` 制造跨包同名前缀。
命令与面板同名不冲突（不同注册表），但会给读者造成困惑。

## 4.7 练习

1. 扩展 id 是 `com.acme.data-tools`，贡献 id 是 `csv.read`，模型看到什么？
2. 一个包只有 prompt 和 skill，没有工具。它能被应用吗？会出现在 `list` 的
   TOOLS 列吗？
3. 面板里一个按钮的 action 是 `refresh`，包里没有同名工具但有同名命令，会发生
   什么？如果运行时是 declarative 呢？
4. 为什么 `command` 既可能是内容型也可能是执行型？

---

<a id="ch5"></a>
# 第 5 章 工具

**学习目标**：写出一个模型愿意调用、宿主能授权、运行时能执行的工具；理解描述符、
实现、结果与每次调用能力检查。

## 5.1 工具是唯一"模型可调用"的表面

其他贡献影响上下文或 UI；只有 `tool` 会出现在模型的函数列表里。描述符就是契约：

~~~json
{
  "description": "Read the status of one Git path.",
  "input_schema": {
    "type": "object",
    "properties": { "path": { "type": "string" } },
    "required": ["path"]
  },
  "capabilities": ["workspace.read"],
  "implementation": { "kind": "echo_arguments" }
}
~~~

| 字段 | 必需 | 规则 |
|---|---:|---|
| `description` | 否 | 最多 4096 字节；省略时用"扩展名 (id)" |
| `input_schema` | 否 | 必须是 JSON 对象；省略时是空对象 schema |
| `capabilities` | 否 | 每次调用检查，叠加在激活授权之上 |
| `implementation` | 仅 declarative | 三种纯变换之一 |

进程、WASM、Extism、MCP 忽略 `implementation`：它们的工具来自运行时
（第 12–16 章）。

## 5.2 declarative 的三种实现

**回显参数**：

~~~json
{ "implementation": { "kind": "echo_arguments" } }
~~~

**常量**：

~~~json
{ "implementation": { "kind": "constant", "value": { "version": "0.1.0" } } }
~~~

**模板**：只替换顶层 `{{name}}`：

~~~json
{
  "implementation": {
    "kind": "template",
    "text": "hello {{name}} n={{n}} missing={{missing}}"
  }
}
~~~

字符串原样输出，数字/对象按 JSON 文本输出，缺失参数渲染为空。模板不是编程
语言：没有条件、循环、嵌套路径。

## 5.3 结果形状

运行时可以返回任意 JSON。宿主做一次规范化（`ToolResult::from_value`）：

- 返回对象且含 `content` 数组 → 视为 MCP 风格内容块，可选
  `structuredContent` 与 `isError`；
- 返回字符串 → 一个文本块；
- 其他 → 一个 JSON 块。

工具结果的"最简形态"会被保留：只有一个文本块或一个 JSON 值时，写回
transcript 的就是那个值本身。因此 `echo_arguments` 在 `mock` 里打印出裸对象，
而不是 `content` 包装。

应用级失败应返回 `isError: true` 并解释下一步；传输级失败（进程退出、wasm
trap、MCP EOF）由宿主报错，可能关闭会话。

## 5.4 每次调用能力检查

描述符里的 `capabilities` 是最小权限接缝：一个包可以注册十个工具，只让其中
一个申请 `workspace.write`。

调用时宿主：

1. 从参数里解析作用域（`path`、`url`、`secret` 这些名字有特殊含义）；
2. 对描述符里的每个能力询问策略引擎；
3. 每次决策写一行审计；
4. 全部通过才调用运行时。

所以会出现"`mock` 里能跑、真实宿主被拒"：`mock` 给所有能力发了 `any` 授权。

## 5.5 命名是 API

声明 `echo`，模型看到 `hello__echo`。改名会破坏引用旧名字的提示、技能与历史
轨迹。把贡献 id 当公开 API 对待。

## 5.6 实验：三个工具

把 `/tmp/hello-lab` 复制成 `/tmp/tools-lab`，添加三个工具并在清单里声明：

~~~json
{
  "description": "Echo the arguments",
  "input_schema": {"type": "object", "properties": {"text": {"type": "string"}}},
  "implementation": {"kind": "echo_arguments"}
}
~~~

~~~json
{
  "description": "Return the package version",
  "implementation": {"kind": "constant", "value": {"version": "0.1.0"}}
}
~~~

~~~json
{
  "description": "Render a greeting",
  "input_schema": {"type": "object", "properties": {"name": {"type": "string"}, "n": {"type": "integer"}}},
  "implementation": {"kind": "template", "text": "hello {{name}} n={{n}} missing={{missing}}"}
}
~~~

验证并运行：

~~~bash
$BSH ext validate /tmp/tools-lab
$BSH ext mock /tmp/tools-lab echo '{"text":"hi"}'
$BSH ext mock /tmp/tools-lab version '{}'
$BSH ext mock /tmp/tools-lab greet '{"name":"Ada","n":2}'
$BSH ext mock /tmp/tools-lab greet '{}'
~~~

真实输出：

~~~text
{ "text": "hi" }
{ "version": "0.1.0" }
"hello Ada n=2 missing="
"hello  n= missing="
~~~

最后一行演示了模板的两个边界：缺失参数变空串；字符串结果按 JSON 字符串打印
（带引号），不是裸文本。

> 声明 `workspace.read` 并不会让 declarative 工具真的能读文件。它只是让宿主在
> 调用前问一次策略；实现仍然是纯变换。要读文件，用 process / wasm / extism
> （第 12–15 章）。

## 5.7 schema 建议

- 参数名用 `path` / `url` / `secret` 表示资源，作用域提取依赖这些名字；
- 除非故意接受未来字段，否则加 `"additionalProperties": false`；
- 描述用动词开头，模型看的是描述不是 README；
- 不要把秘密写进默认值；
- 一个工具只做一件事，参数越少，模型越不容易填错。

## 5.8 常见错误

| 现象 | 原因 | 处理 |
|---|---|---|
| `declarative tool X needs an implementation` | 声明了工具但 JSON 没有 `implementation` | 加三种实现之一 |
| `input_schema must be a JSON object` | schema 写成数组/字符串 | 改成对象 |
| `tool result is N bytes; the ceiling is M` | 输出超限 | 改 `max_output_bytes` 或裁剪结果 |
| 模型看不到工具 | 包未应用 / TOOLS 列为空 | `why`、`list` |
| 调用被拒 | 能力不足或作用域不匹配 | 看 audit 与 `info` 的 capability 行 |

## 5.9 练习

1. 写一个 `sum` 工具，输入两个整数，返回它们的和。declarative 能完成吗？
2. 给 `greet` 加上默认值语义（缺 `name` 时用 "world"）。模板能表达吗？不能的话，
   需要哪种运行时？
3. 为什么 `capabilities` 放在工具描述符而不是只在清单里？

---

<a id="ch6"></a>
# 第 6 章 命令、提示、上下文与技能

**学习目标**：用 Markdown 贡献影响"读者能插入什么"与"模型知道什么"；理解
frontmatter、作用域与注入预算。

这四种表面都不增加可调用工具，但会显著改变 agent 的行为。

## 6.1 命令（command）

~~~markdown
---
description: Greet the reader and name the extension
---

Say hello from com.example.hello and name the open workspace.
~~~

- 文件：`commands/<id>.md`；
- frontmatter 只读两个键：`description`、`name`（未知键忽略）；
- 正文是**插入到输入框的模板**，不是自动执行；
- 没有 `description` 时取正文第一行（去掉 `#`）作为描述；
- 清单要声明 `kind = "command"`。

命令面板里显示的是资格名 `hello__greet`，描述来自 frontmatter。对内容型命令，
宿主只负责插入模板；而运行时的 `command()` 接缝是另一条路（面板动作回退、
集成动作、`bsh ext integrate`）。declarative 运行时没有 `command()` 实现，
所以指向 declarative 命令的集成动作会失败——这是设计边界，不是 bug。

## 6.2 提示（prompt）

~~~markdown
---
description: Hello prompt fragment
---

When the reader asks what this extension does, say that it is the BSH hello
example: an echo tool, a greet command and a status panel.
~~~

- 文件：`prompts/<id>.md`；
- 正文追加到系统提示；
- 单段上限 8 KiB，一个构建里所有扩展的提示总预算 64 KiB；
- 超出预算的部分会被截断或跳过（先到先得，按安装/构建顺序）。

## 6.3 上下文（context）

~~~markdown
---
description: All-in-one demo scoped workspace context
paths: crates/**/*.rs, src/**/*.rs
---

This workspace contains Rust crates. ...
~~~

- 文件：`contexts/<id>.md`；
- frontmatter 的 `paths:` 用逗号分隔，写工作区相对 glob；
- 没有 `paths:` 时始终追加；有匹配文件时才追加；
- 与 prompt 共享同样的预算。

**prompt 与 context 的选择**：稳定的角色/规则用 prompt；只在某些目录下成立的
事实用 context。

## 6.4 技能（skill）

~~~text
skills/allinone/SKILL.md
~~~

~~~markdown
---
name: allinone
description: Walk the reader through the BSH extension framework
version: 1.0.0
---

# All-in-One Demo

Use this skill when ...
~~~

- 清单声明 `kind = "skill" id = "allinone"`；
- 目录名是贡献 id，里面必须有 `SKILL.md`；
- 宿主把 `skills/` 加入 agent 的技能搜索路径；
- 技能的触发与加载由 agent 的技能机制负责（描述写得越具体，越容易被正确触发）。

技能最多 256 个条目。技能加载失败会记日志，但不会让整个包失败——这是内容型
贡献与执行型贡献的差别。

## 6.5 这些表面怎样进入一轮对话

~~~text
构建 agent
  ├─ prompt 段（始终）
  ├─ context 段（按 paths 过滤）
  └─ skills 目录（供技能加载）
       │
       ▼
系统提示 + 用户输入 → 模型
       │
命令面板：读者插入 command 模板（不经过模型预先执行）
~~~

## 6.6 实验：加一条命令与一个上下文

在 `/tmp/hello-lab` 里：

1. 新建 `commands/review.md`：

   ~~~markdown
   ---
   description: Review the current file
   ---

   Review the file I am looking at and list three concrete risks.
   ~~~

2. 新建 `contexts/rust.md`：

   ~~~markdown
   ---
   description: Rust workspace facts
   paths: crates/**/*.rs
   ---

   This workspace is a Rust workspace with one crate per capability layer.
   ~~~

3. 在清单里声明：

   ~~~toml
   [[contribution]]
   kind = "command"
   id = "review"

   [[contribution]]
   kind = "context"
   id = "rust"
   ~~~

4. 运行：

   ~~~bash
   $BSH ext validate /tmp/hello-lab
   $BSH ext test /tmp/hello-lab
   $BSH ext install /tmp/hello-lab --root /tmp/hello-root
   $BSH ext info com.example.hello --root /tmp/hello-root
   ~~~

`info` 的 `commands` 行会出现 `hello__review`。

## 6.7 常见错误

| 现象 | 原因 | 处理 |
|---|---|---|
| 命令不显示 | 清单没声明 / 包未应用 | 补 `[[contribution]]`，看 `info` |
| 描述是正文第一行 | 没写 frontmatter description | 加 frontmatter |
| frontmatter 没生效 | `---` 不在文件第一行，或没有闭合 | 从第一行开始，允许 BOM |
| 上下文总是出现 | 漏了 `paths:` | 加 `paths:` |
| 提示被截断 | 超过 8 KiB / 64 KiB 预算 | 压缩或拆成 context |
| 技能不触发 | 描述太泛 | 写清"何时使用" |

## 6.8 练习

1. 把 `contexts/rust.md` 的 `paths` 改成只匹配 `docs/**/*.md`，它还会出现吗？
2. 为什么命令正文不直接执行，而是插入输入框？这对安全意味着什么？
3. 写一个技能，让 agent 在"用户要求审查扩展清单"时加载它。

---

<a id="ch7"></a>
# 第 7 章 面板与声明式 UI

**学习目标**：写一个能被宿主校验、能挂载、能派发动作的面板；知道节点词表与限制；
能诊断"面板空白"。

## 7.1 面板文件

~~~json
{
  "title": "Hello",
  "view": {
    "schema_version": "1.0",
    "root": {
      "type": "column",
      "children": [
        { "type": "heading", "text": "Hello", "level": 2 },
        { "type": "text", "text": "The declarative extension is active." },
        { "type": "button", "label": "Echo", "action": "echo", "style": "primary" }
      ]
    }
  }
}
~~~

| 字段 | 必需 | 规则 |
|---|---:|---|
| `title` | 是 | 1–120 字符 |
| `icon` | 否 | 包相对路径 |
| `location` | 否 | `right`（默认）/`left`/`bottom`/`center` |
| `default_visibility` | 否 | `visible`（默认）/`hidden` |
| `view` | 是 | UI 文档 |

UI 文档的形状固定为 `{schema_version, root}`，`schema_version` 必须是
`"1.0"`。动态运行时返回的文档里可以带 `revision` 之类的额外键，解析时会忽略；
**修订号由宿主管理**，不是文档的一部分。

## 7.2 节点词表

节点的 `type` 用 kebab-case：

| type | 关键字段 | 说明 |
|---|---|---|
| `container` / `row` / `column` | `children` | 布局容器 |
| `text` | `text` | 正文 |
| `heading` | `text`、`level`（1–6，默认 2） | 标题 |
| `markdown` | `text` | Markdown 渲染 |
| `button` | `label`、`action`、`style` | 动作 |
| `input` | `label`、`value`、`placeholder`、`action` | 单行输入，提交派发动作 |
| `select` | `label`、`value`、`options`、`action` | 选项为 `{value,label}` |
| `checkbox` | `label`、`checked`、`action` | 勾选 |
| `list` | `items` | 行 `{title,detail,action?,checked}` |
| `table` | `columns`、`rows` | 单元格是字符串 |
| `tree` | `nodes` | `{id,label,children}` |
| `tabs` | `tabs` | `{id,label,content}`，content 是节点 |
| `progress` | `value`、`label` | value 0.0–1.0 |
| `code` | `language`、`value` | 代码块 |
| `image` | `src`、`alt` | **只画 `asset/...` 包内路径** |
| `notification` | `level`、`text` | info / warning / error / success |
| `divider` | 无 | 分隔线 |
| `spacer` | `size` | 默认 8 |

校验上限（`UiLimits::default`）：深度 32、节点 1000、单文本 16 KiB、总文本
256 KiB、列表 500 项、表格 500 行 × 32 列、单元格 2048 字符、标签 16 个、
序列化文档 1 MiB。

## 7.3 动作怎样派发

点击 `action = "echo"` 的按钮：

~~~text
面板动作
  │ 1. 若包里有同名工具描述符：按该描述符的 capabilities 做每次调用检查
  │ 2. 发 ui.action 事件（扩展 id、面板 id、动作、修订号）
  │ 3. runtime.call_tool("echo", payload, ctx)
  │      若返回 NotFound（没有这个工具）
  │      → runtime.command("echo", payload)（回退）
  ▼
结果写回面板消息区；失败显示错误
~~~

两个推论：

- **动作名是贡献 id，不是资格名**。面板里写 `echo`，不是 `hello__echo`。
- **declarative 面板的动作如果要"做事"，必须指向工具**；它没有 `command()`
  实现，回退会得到 `UnsupportedRuntime`。

按钮 `style` 取 `primary` / `secondary`（默认）/ `danger` / `ghost`。

## 7.4 静态面板与动态面板

| 类型 | 来源 | 何时出现 | 能否刷新 |
|---|---|---|---|
| 静态 | `panels/<id>.json` | 包一应用就有 | 否 |
| 动态 | 运行时 `panel()`（Extism `bsh_panel`、WASM `panel`） | 面板第一次加载时调用 | 重新加载时再调 |

静态面板的 `initial` 视图就是文件内容；动态面板的返回值必须是合法 UI 文档，
否则显示 `panel document is not valid`。

## 7.5 实验：给 Hello 面板加一个动作

1. 确认 `tools/echo.json` 存在并声明；
2. 在 `panels/status.json` 里放一个按钮：

   ~~~json
   { "type": "button", "label": "Echo", "action": "echo", "style": "primary" }
   ~~~

3. 安装并应用：

   ~~~bash
   $BSH ext install /tmp/hello-lab --root /tmp/hello-root
   $BSH ext info com.example.hello --root /tmp/hello-root
   ~~~

4. 在 BSH 里打开该面板，点 Echo。声明式 `echo_arguments` 在没有任何输入时会
   返回 `{}`；这证明"动作 → 工具 → 运行时"整条链路通了。

## 7.6 空白面板排查

| 现象 | 原因 | 处理 |
|---|---|---|
| 面板根本不在列表 | 包未应用 / `ui.panels` 未声明 | `why`、`info` |
| 面板出现但空白 | `view` 校验失败 | 看 `ext validate`；错误字段从 `ui.` 开始 |
| 图片不显示 | `src` 不是 `asset/...` | 把图片放进 `asset/` |
| 按钮点了没反应 | 没有同名工具/命令 | 补工具，或改用运行时命令 |
| 报 stale revision | 动态视图修订号没有推进 | 重新加载；不要伪造修订号 |
| 面板标题乱码/超长 | 超过 120 字符 | 缩短 |

## 7.7 练习

1. 用 `table` 显示三行两列的状态数据。
2. 给面板加一个 `input`，提交时把内容传给 `echo`；payload 会是什么形状？
3. 为什么远程图片被拒绝？如果允许，会引入什么风险？

---

<a id="ch8"></a>
# 第 8 章 事件

**学习目标**：理解事件信封与主题语法；知道保留主题、限额与声明方式；能判断
什么时候用事件、什么时候用钩子。

## 8.1 信封

~~~json
{
  "id": "uuid",
  "source": "com.example.hello",
  "event_type": "workspace.changed",
  "schema_version": "1.0",
  "timestamp": 1793702405000,
  "correlation_id": "turn-3",
  "payload": { "path": "src/main.rs" }
}
~~~

| 字段 | 说明 |
|---|---|
| `id` | 事件唯一 id |
| `source` | 发布者；宿主事件是 `"bsh"` |
| `event_type` | 点分主题 |
| `schema_version` | 信封代际，当前 `"1.0"` |
| `timestamp` | Unix 毫秒 |
| `correlation_id` | 可选，关联到一次请求/回合 |
| `payload` | 任意 JSON，深度 ≤ 32 |

## 8.2 主题语法

- 点分；每段只允许小写字母、数字、`-`、`_`；
- 总长 ≤ 128 字节；
- 订阅模式只允许**最后一段**是 `*`（如 `tool.*`）；
- 事件发布时不允许通配。

## 8.3 宿主保留主题

~~~text
app.started / app.shutdown
workspace.opened / workspace.closed / workspace.changed
agent.started / agent.completed / agent.failed
tool.invoked / tool.completed / tool.failed
extension.loaded / extension.unloaded / extension.failed
ui.action
model.requested / model.completed
config.changed
session.created / session.updated / session.deleted
trajectory.recorded
skill.loaded
instruction.build
network.request
mcp.connected
audio.captured
~~~

扩展可以订阅它们，也可以发布自己的主题。发布到 `tool.*` 是一个特例：
Extism 桥允许它，无需在清单里声明（仍需要 `events.publish` 能力）。

## 8.4 声明订阅与发布

~~~toml
[[contribution]]
kind = "event.subscribe"
id = "workspace.changed"

[[contribution]]
kind = "event.publish"
id = "hello.refreshed"
~~~

- 订阅：只有声明过的主题（或匹配的模式）才能通过桥订阅；
- 发布：只有声明过的主题（或 `tool.*`）才能通过桥发布；
- 声明上限：每类 256 个主题；
- 能力：`events.subscribe` / `events.publish`，都是低风险。

> 代码里的报错仍使用 1.x 措辞（`contributes.publishes` /
> `contributes.events`）。看到它们时按 2.0 的 contribution 理解即可。

## 8.5 限额与投递语义

| 限制 | 值 |
|---|---|
| 单个事件序列化上限 | 256 KiB |
| 每个 source 每秒事件数 | 200 |
| 订阅队列深度（默认） | 64 |
| payload 深度 | 32 |

超过速率会报错（不是静默丢弃）；队列满时按订阅策略丢弃并计数。
`bsh ext metrics` 会给出 `events_published`、`events_delivered`、
`events_dropped`、`subscribers`。

事件是"事后通知"：投递失败不会回滚已经发生的事。不要用它实现"拦截"。

## 8.6 UI 动作也是事件

面板动作派发时，宿主先发 `ui.action`：

~~~json
{
  "extension": "com.example.hello",
  "panel": "hello__status",
  "action": "echo",
  "revision": 1
}
~~~

这给审计与观测一个统一入口；它不替代每次调用能力检查。

## 8.7 实验：订阅工作区变化

1. 在清单里加：

   ~~~toml
   [[contribution]]
   kind = "event.subscribe"
   id = "workspace.changed"
   ~~~

2. 加所需能力 `events.subscribe`；
3. 安装后运行：

   ~~~bash
   $BSH ext install /tmp/hello-lab --root /tmp/hello-root
   $BSH ext info com.example.hello --root /tmp/hello-root
   $BSH ext metrics --root /tmp/hello-root
   ~~~

在 Extism 包里，桥的 `subscribe` 会检查声明；进程包则在运行时启动时按声明
订阅（第 12 章）。

## 8.8 事件 vs 钩子：选择表

| 你想做 | 用事件 | 用钩子 |
|---|---:|---:|
| 通知别的组件"发生了什么" | 是 | 否 |
| 记录审计、刷新 UI | 是 | 是（更细） |
| 拿到 prompt/response 的形状 | 否 | 是 |
| 阻止或改写一次调用 | 否 | 当前都不行（intercept 未实现） |
| 跨进程/跨扩展广播 | 是 | 否 |

## 8.9 练习

1. 主题 `Tool.Completed` 合法吗？`tool.*.completed` 呢？
2. 一个扩展每秒发布 500 个事件，会发生什么？
3. 为什么 `ui.action` 不能替代工具调用授权？

---

<a id="ch9"></a>
# 第 9 章 配置、存储与描述性贡献

**学习目标**：给扩展加配置 schema；知道数据存哪里；能写出主题、预设、提供方、
语言服务、集成与工作流描述符。

## 9.1 配置

~~~toml
[configuration]
schema = "schemas/config.schema.json"

[configuration.defaults]
greeting = "hello"
verbose = false
~~~

- `defaults` 是 TOML，读入时转成 JSON；
- 用户配置存放在 `extension-data/<id>/config.json`；
- 有用户文件时用它，否则用 defaults；
- 保存时按包里 schema 校验（一个受支持的 JSON Schema 子集）；
- 运行时通过握手拿到配置 JSON：process 的 `initialize.params.config`、
  Extism 的 `{"op":"config"}`、WASM 的 `initialize(config)`。

CLI：

~~~bash
$BSH ext config com.example.hello --root /tmp/hello-root          # 读取
$BSH ext config com.example.hello schema --root /tmp/hello-root   # 打印 schema
$BSH ext config com.example.hello set '{"greeting":"hi"}' --root /tmp/hello-root
~~~

> **现状提醒**：3.0.2 的 `ext config` 用 1.x 清单解析器读已安装的 2.0 包，会报
> `unknown field host`。在修复前，可以直接写 `extension-data/<id>/config.json`
> 验证运行时行为；详见附录 I。

## 9.2 存储

每个扩展有独立数据目录 `extension-data/<id>/`。宿主不把不同扩展的键混在一起。

| 运行时 | 访问方式 |
|---|---|
| Extism | `storage_get` / `storage_set` / `storage_list` 桥操作 |
| WASM | WIT 的 `storage-get` / `storage-set` / `storage-list` |
| process | 自己的代码 + 数据目录（没有宿主存储 API，也没有操作系统沙箱） |
| Cordis | 通过桥暴露的宿主调用（以桥实现为准） |

存储能力是 `extension.storage`（低风险）。键名要自己命名空间化（例如
`counts`、`aggs/latency.json`），宿主不会帮你分前缀。

## 9.3 主题（theme）

~~~json
{
  "name": "All-in-One Aurora",
  "colors": {
    "page": "#0b0d10",
    "text": "#e6e8eb",
    "muted": "#9aa0a6",
    "accent": "#4176e6",
    "border": "#2a2f36",
    "new-session-fill": "#171a1f"
  }
}
~~~

- `colors` 的键是 kebab-case 颜色 token，值是 `#RRGGBB` 或 `#RRGGBBAA`；
- 宿主只解析它认识的 token，未知的忽略；认识的 token 缺失时回退到宿主调色板；
- 主题只作用于该扩展自己的面板（在面板渲染时选用）。

## 9.4 预设（preset）与提供方（provider）

预设（示例）：

~~~json
{
  "name": "All-in-One Safe",
  "description": "A read-only preset",
  "level": 1,
  "permission": "read_only",
  "plugins": ["files"]
}
~~~

模型提供方（示例）：

~~~json
{
  "name": "All-in-One Example",
  "base_url": "https://example.invalid/v1",
  "api_mode": "chat",
  "demo-model": { "model": "demo-model", "name": "Demo Model" }
}
~~~

agent 角色（示例）：

~~~json
{
  "name": "All-in-One Helper",
  "description": "A read-only agent role",
  "plugins": ["files"],
  "permission": "read_only",
  "level": 1
}
~~~

这些是**记录型描述符**：宿主读取它认识的字段，未知字段被保留但不解释。
它们不会获得额外权限，也不会自动成为默认值。

## 9.5 语言服务

~~~json
{
  "name": "All-in-One Demo Language",
  "extensions": ["allinone"],
  "tool": "describe",
  "syntax": "rust"
}
~~~

校验规则：

- `name` 1–120 字符；
- `extensions` 1–64 个，**不带点**，只允许字母数字与 `_-+.`；
- `tool` 必须是已声明的工具；
- `syntax` 可选。

运行诊断：

~~~bash
$BSH ext language-check com.example.allinone demo-language src/main.rs \
  --root /tmp/hello-root
~~~

宿主读取文档（上限 1 MiB），把 `{path, text, language}` 传给该工具，并打印
工具返回的 JSON。

## 9.6 集成

~~~json
{
  "name": "All-in-One GitHub",
  "kind": "github",
  "action": "describe"
}
~~~

- `kind` ∈ `github` / `browser` / `terminal` / `custom`；
- `action` 必须是已声明的**工具或命令**；
- 运行：

  ~~~bash
  $BSH ext integrate com.example.allinone github '{"q":"extensions"}' \
    --root /tmp/hello-root
  ~~~

宿主先当工具调用，失败再当运行时命令；两者都受能力检查。

## 9.7 工作流

~~~json
{
  "description": "Inspect the demo",
  "input_schema": { "type": "object", "properties": { "limit": { "type": "integer" } } },
  "steps": [
    { "name": "about", "tool": "describe", "arguments": {} },
    { "name": "log", "tool": "events", "arguments": { "limit": "{{input.limit}}" } }
  ]
}
~~~

- 工作流包装成**一个工具**，工具 id 是工作流 id；
- 步骤只能调用**同一个包**里的工具；
- 参数支持 `{{input.x}}` 与 `{{steps.name.x}}`；
- 每一步在执行前按解析后的参数做能力检查；
- 工作流本身不聚合能力列表，避免"用外层输入授权内层资源"。

## 9.8 实验：配置驱动的问候

1. 加 `schemas/config.schema.json` 与 defaults；
2. 在 process/Extism 工具里读配置并改变输出；
3. 用 `bsh ext config set`（或直接写 `config.json`）改值，观察工具输出变化；
4. 把配置改成 schema 不允许的类型，观察保存拒绝。

## 9.9 练习

1. 主题里写了 `"page": "red"` 会怎样？
2. 工作流步骤引用了另一个包的工具，什么时候报错？
3. 为什么配置默认值不能放秘密？

---

<a id="ch10"></a>
# 第 10 章 运行时总览与选择

**学习目标**：用一张决策表选出合适运行时；知道每个运行时的能力边界与代价。

## 10.1 七个运行时

| 运行时 | 代码形态 | 工具描述来自 | 面板 | 命令 | 事件 | 钩子 |
|---|---|---|---|---|---|---|
| `builtin` | 链入宿主的 Rust | 运行时注册 | 视实现 | 视实现 | 视实现 | 视实现 |
| `declarative` | 包内文件 | `tools/<id>.json` | 静态 | 模板（无运行时命令） | 无 | 无 |
| `process` | 子进程（JSON-RPC） | `tools/<id>.json` | 无 | `commands/execute` | `events/event` 通知 | 无 |
| `mcp` | MCP server | `tools/list` | 无 | 无 | 视 server | 无 |
| `extism` | Extism 插件 | `bsh_tools`（回退文件） | `bsh_panel` | `bsh_command` | `bsh_event` | `bsh_hook` |
| `wasm-component` | Component | `tools()` 导出 | `panel` | `command` | `on-event` | 无 |
| `cordis` | `apply(ctx, config)` JS | 桥注册 | 无 | 桥注册 | 桥监听 | 桥 `hook`（清单未开） |

## 10.2 决策顺序

先问四个问题：

1. **需要执行代码吗？**
   不需要 → `declarative`。需要 → 继续。
2. **需要操作系统能力（文件、网络、进程、依赖）吗？**
   需要且能用沙箱/子进程 → `process`（最灵活）。
   不能起进程、要强隔离 → `extism` / `wasm-component`。
3. **已有 MCP server 吗？**
   有 → `mcp`（适配成本最低）。
4. **已有 Cordis/Node 包吗？**
   有 → `cordis`。

补充判断：

- 需要**观测钩子** → 只能 `extism`（本构建）。
- 需要**强类型契约、可移植** → `wasm-component`。
- 需要**多语言依赖**（Python 库、CLI 工具）→ `process`。
- 需要**最快迭代** → `declarative` 或 `process`（改文件/改脚本即可）。

## 10.3 代价对比

| 维度 | declarative | process | extism | wasm | mcp | cordis |
|---|---|---|---|---|---|---|
| 启动成本 | 无 | 进程启动 + 握手 | wasm 实例 | wasm 编译 | 进程 + 握手 | Node 启动 |
| 调用开销 | 极低 | 一次 IPC | 一次 wasm 调用 | 一次 wasm 调用 | 一次 IPC | 一次 IPC |
| 隔离 | 文件解析 | OS 沙箱 + rlimit | wasm 沙箱 | wasm 沙箱 | OS 沙箱 | Node 沙箱 |
| 调试 | 最简单 | 中 | 较难 | 较难 | 中 | 中 |
| 能力粒度 | 描述符 | 描述符 + 进程 | 描述符 + 桥 | host 导入 | 服务器 | 桥 |

## 10.4 启动策略与懒激活

`runtime.config.startup`：

- `on-startup`：宿主启动时激活；
- `on-demand`（默认）：第一次用到时激活；
- `on-workspace-open`：工作区打开时；
- `manual`：只能显式触发。

宿主用 `LazyRuntime` 包住运行时：第一次调用才 `activate()`，失败后按
50/100/200/400 ms 退避重试（上限 400 ms）。所以"装好但没启动"是正常的；
第一次工具调用会付出启动成本。

## 10.5 运行时不等于信任

运行时决定**怎么执行**，能力决定**能碰什么**。一个 `process` 包没有
`process.execute` 就不能起进程；一个 `extism` 包没有 `network.http` 就不能
`fetch`。不要用"选了更安全的运行时"代替能力审查。

## 10.6 练习

1. 你要做一个读 Git 状态、写文件的工具包，选哪个运行时？为什么？
2. 你要复用公司已有的 MCP server，选哪个？
3. 你要做 LLM 观测器，选哪个？如果必须用 process 呢？

---

<a id="ch11"></a>
# 第 11 章 declarative 运行时

**学习目标**：掌握"无代码扩展"的全部能力与边界；知道什么时候必须换运行时。

## 11.1 它做什么

`declarative` 把包目录当成程序：

- 工具体是三种纯变换（第 5 章）；
- 命令/提示/上下文是 Markdown；
- 面板是静态 JSON；
- workflow/theme/preset/provider/language/integration 是描述符文件。

它**不执行任何代码**：没有文件、网络、进程、秘密访问。安全边界是解析器与 UI
校验器。

## 11.2 强制一致性

每个声明的工具都必须有 `tools/<id>.json`，且必须有 `implementation`。
声明了却没提供，整个包在加载时被拒绝：

~~~text
declarative tool echo needs an implementation
~~~

这是刻意的：清单是契约，不能"少给一个工具"悄悄通过。

## 11.3 工具体的边界

| 能力 | 支持 |
|---|---|
| 读参数 | 是（回显/模板） |
| 拼字符串 | 是（顶层占位符） |
| 条件/循环 | 否 |
| 读文件/网络 | 否 |
| 保存状态 | 否（除了宿主配置） |

需要上述任何一项，换 `process` / `extism` / `wasm-component`。

## 11.4 热重载

`bsh ext reload <id>` 重建宿主对该扩展的视图。它不是文件监视器：改完文件要
显式 reload（或重新安装）。开发循环通常是：

~~~bash
$BSH ext validate /tmp/lab && $BSH ext reload com.example.hello --root /tmp/root
~~~

## 11.5 什么时候够用

- 静态知识/提示包；
- 把固定 JSON 暴露成工具；
- 把模板变成"伪工具"做演示；
- 面板原型；
- 只想分发描述符（主题、预设、语言、集成）。

## 11.6 实验

用第 5 章的 `/tmp/tools-lab` 完成：

1. 加一个 `constant` 工具返回当前课程编号；
2. 加一个 `template` 工具渲染 `{{path}}`；
3. 用 `validate`、`mock`、`test` 各跑一遍；
4. 故意删掉一个 implementation，读错误。

---

<a id="ch12"></a>
# 第 12 章 process 运行时

**学习目标**：写一个符合协议的进程扩展；理解握手、行协议、沙箱与限额；能排查
握手失败。

## 12.1 什么时候用

- 需要 Python/Node/Go 等语言的依赖；
- 需要语言服务、编译、网络客户端；
- 需要比 wasm 更宽松的系统能力，但仍要被沙箱与超时约束。

代价：一次进程启动 + 每调用一次 IPC；进程崩溃会影响该扩展的调用。

## 12.2 清单

~~~toml
[runtime]
kind = "process"

[runtime.config]
entry = "python3"
args = ["server.py"]
protocol = "jsonrpc-2.0"
startup = "on-demand"

[runtime.config.env]
LOG_LEVEL = "info"
~~~

- `entry` 要么是白名单裸命令（`python3`、`python`、`node`、`bun`、
  `deno`、`sh`、`bash`），要么是**包相对**可执行文件；绝对路径会被拒绝。
- `protocol` 必须是 `"jsonrpc-2.0"`。
- `args` 是参数数组；`env` 追加环境变量。

## 12.3 启动与握手

宿主：

- 以包根为工作目录；
- **清空环境**，只保留 `PATH`、`HOME`、`TMPDIR`、`LANG`、`LC_ALL`，
  加上 `runtime.config.env`、`BSH_IPC_TOKEN` 和 `BSH_BIN`（宿主可执行文件路径，
  扩展可直接调用 `bsh ext ...`）；
- stdout/stdin 管道，stderr 记日志；
- 不经过 shell；
- Linux 用 `bwrap`、macOS 用 `sandbox-exec` 做文件系统沙箱（默认开启）；
- 设置 CPU/文件描述符 rlimit，并启动内存看门狗。

握手请求（一行）：

~~~json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
  "protocolVersion":"1.0",
  "extensionId":"com.example.process-echo",
  "host":{"name":"bsh","version":"3.0.2"},
  "config":"{}",
  "token":"<uuid>"
}}
~~~

响应必须：

- `protocolVersion` 以 `"1."` 开头；
- `token` 原样回显。

否则宿主动手杀进程并报错。握手超时 10 秒。

## 12.4 行协议

- 一行一个 JSON 对象；行上限 4 MiB；
- 请求带 `id`，响应带同一个 `id`；
- 工具调用：

  ~~~json
  {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
    "name":"echo","arguments":{"text":"hi"},"correlationId":"..."
  }}
  ~~~

- 命令调用：`commands/execute`，参数 `{name, arguments}`；
- 关闭：`shutdown`，宿主只等 1 秒，然后杀进程；
- 进度：服务端发 `$/progress` 通知（无 `id`），宿主转发给调用方；
- 事件：宿主通过 `events/event` 通知投递订阅事件，参数 `{event}`。

响应形状：

~~~json
{"jsonrpc":"2.0","id":2,"result":{"text":"hi"}}
{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"boom"}}
~~~

## 12.5 一个完整 server

~~~python
import json
import sys


def main() -> None:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = request.get("method")
        if method == "initialize":
            result = {
                "protocolVersion": "1.0",
                "token": request.get("params", {}).get("token"),
            }
        elif method == "tools/call":
            result = request.get("params", {}).get("arguments", {})
        elif method == "commands/execute":
            result = {"id": request.get("params", {}).get("name")}
        elif method == "shutdown":
            result = {}
        else:
            result = {}
        print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)


if __name__ == "__main__":
    main()
~~~

手工验证协议（不经过宿主）：

~~~bash
cd examples/extensions/process-echo
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"token":"abc"}}' | python3 server.py
~~~

~~~text
{"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": "1.0", "token": "abc"}}
~~~

## 12.6 工具描述符

进程运行时不从子进程拿工具列表：它在启动前读 `tools/<id>.json`。所以：

- 文件必须存在；
- `description` / `input_schema` / `capabilities` 以文件为准；
- 子进程只需要实现 `tools/call`。

## 12.7 限额与沙箱

- 每次调用超时取 `CallContext.timeout_ms` 与清单 `max_execution_ms` 的较小值；
- 输出上限 `max_output_bytes`；
- 内存上限 `max_memory_mb`（Linux `RLIMIT_AS` + 看门狗；macOS 看门狗）；
- CPU 上限由执行时间推算；
- 文件系统：Linux `bwrap`、macOS `sandbox-exec`，只允许写包根、工作区、
  数据目录、临时目录/设备。

> **环境提醒**：如果 BSH 自己已经跑在文件沙箱里（例如某些 CI 或受限终端），
> 嵌套的 `sandbox-exec` 会返回 `sandbox_apply: Operation not permitted`，表现为
> `process closed stdout before answering`。这不是协议 bug。正常桌面环境不会这样。

## 12.8 排查握手失败

| 现象 | 原因 | 处理 |
|---|---|---|
| `process closed stdout before answering` | 子进程没起来/立刻退出/被外层沙箱拒绝 | 手工跑 server；检查 shebang、可执行位、依赖 |
| `process speaks protocol "..."` | 返回了别的版本 | 返回 `"1.0"` |
| `did not echo the handshake token` | 忘了回显 `params.token` | 原样返回 |
| `timed out after 10000 ms` | 启动太慢/死锁 | 减少初始化工作 |
| 调用超时 | 单次执行超过 `max_execution_ms` | 调整限额或异步化 |
| 工具不存在 | `tools/<id>.json` 缺失或 id 不匹配 | 对齐清单与文件 |

## 12.9 练习

1. 给 server 加一个 `sum` 工具；
2. 让 server 在 `tools/call` 前发一条 `$/progress`，观察 `mock` 的 stderr；
3. 把 `entry` 改成绝对路径 `/bin/sh`，读校验错误。

---

<a id="ch13"></a>
# 第 13 章 MCP 运行时

**学习目标**：把一个现成 MCP server 接成 BSH 扩展；知道工具、资源、提示如何映射；
理解其限额与信任边界。

## 13.1 它是什么

MCP 运行时是**适配器**：它启动 server，完成 MCP 握手，把 server 的
`tools/list`、`resources/list`、`prompts/list` 映射成 BSH 的工具、上下文、
提示。MCP 类型不会进入 Agent API。

## 13.2 清单

~~~toml
[runtime]
kind = "mcp"

[runtime.config]
entry = "python3"
args = ["server.py"]
protocol = "mcp"
startup = "on-demand"

[capabilities]
required = ["process.execute", "agent.tools.register"]

[[contribution]]
kind = "prompt"
id = "greet"

[[contribution]]
kind = "context"
id = "guide"
~~~

- `protocol` 必须是 `"mcp"`；
- server 通过 stdio 通信，stdout 只能有协议 JSON（一行一个）；
- 需要 `process.execute`（启动子进程）与 `agent.tools.register`（注册工具）。

## 13.3 握手与发现

~~~text
initialize（协议 2024-11-05）
  → notifications/initialized
  → tools/list（分页，最多 256 个）
  → resources/list（最多 256 个）→ contexts
  → prompts/list + prompts/get（最多 256 个）→ prompts
~~~

- 工具名会被 sanitize 成贡献 id；两个名字撞到同一个 id 时保留第一个并记日志；
- 工具的 `capabilities` 是空列表：MCP 工具不按描述符做每次调用能力检查；
  它的权限来自"能启动这个 server"以及 server 自己能做什么；
- 发现的 prompt/context 是**内容型贡献**，追加到 agent 上下文；
- 清单里声明的 prompt/context 不会额外读文件（MCP 工厂不调用
  `load_contributions`）；真正的来源是 server。

## 13.4 调用

`tools/call` 转发到 server，结果按 MCP 内容块解析（`content`、
`structuredContent`、`isError`）。超时取调用上下文与 `max_execution_ms` 的
较小值；超时会杀掉子进程，让阻塞读立刻返回。

MCP transport 是同步 stdio，调用放在 blocking 线程池里，避免堵住 executor。

## 13.5 限额

| 限制 | 值 |
|---|---|
| 握手超时 | 10 秒 |
| 单行响应 | 4 MiB |
| 每类发现条目 | 256 |
| 调用超时 | min(调用上下文, max_execution_ms) |

## 13.6 实验

以 `examples/extensions/mcp-echo` 为例：

~~~bash
$BSH ext validate examples/extensions/mcp-echo
$BSH ext install examples/extensions/mcp-echo --root /tmp/hello-root
$BSH ext info com.example.mcp-echo --root /tmp/hello-root
~~~

如果环境允许起 Python 子进程，`info` 的 `tools` 行会出现 `mcp-echo__echo`。
在受限沙箱里可能显示 `failed`，原因见第 12.7 节。

## 13.7 常见错误

| 现象 | 原因 | 处理 |
|---|---|---|
| 握手超时 | server 没按 MCP 协议回 `initialize` | 用 MCP 客户端先单独验证 |
| `protocol` 校验失败 | 写了 `jsonrpc-2.0` | 改成 `"mcp"` |
| stdout 混入日志 | server 往 stdout 打日志 | 日志走 stderr |
| 工具名重复 | 两个 server 工具 sanitize 后同名 | 重命名其中一个 |
| 调用被杀 | 超时 | 提高 `max_execution_ms` 或优化 server |

## 13.8 练习

1. 用官方 MCP echo server 接一个包；
2. 让 server 暴露一个 resource，验证它成为 context；
3. 思考：MCP 工具的"每次调用能力检查"缺位，会带来什么风险？如何补偿？

---

<a id="ch14"></a>
# 第 14 章 Extism 运行时

**学习目标**：写出一个 Extism 插件，正确使用宿主桥；理解工具/面板/命令/事件/
钩子的导出契约。

Extism 是当前唯一能接收观测钩子的运行时，所以这一章也是第 23 章的基础。

## 14.1 清单

~~~toml
[runtime]
kind = "extism"

[runtime.config]
entry = "plugin.wasm"
startup = "on-demand"

[capabilities]
required = ["agent.tools.register", "extension.storage"]

[[contribution]]
kind = "tool"
id = "echo"
~~~

- `entry` 是包相对 wasm 文件；
- 清单声明的工具必须有 `tools/<id>.json`（构造时读取）；如果插件导出
  `bsh_tools`，它的输出会取代文件描述符（宿主构建视图时调用）；
- 声明了 command/panel 时，仍需 `commands/<id>.md` / `panels/<id>.json`
  （Extism 工厂也会加载声明式贡献）。

## 14.2 导出

| 导出 | 输入 | 输出 | 用途 |
|---|---|---|---|
| `bsh_tools` | `{}` | 工具数组 | 覆盖描述符 |
| `bsh_call_tool` | `{name, arguments, correlationId, config}` | 任意 JSON | 工具调用 |
| `bsh_command` | `{id, arguments}` | 任意 JSON | 运行时命令 |
| `bsh_panel` | `{id}` | UI 文档 | 动态面板 |
| `bsh_event` | 事件 JSON | 忽略 | 事件投递 |
| `bsh_hook` | `{point, batch}` | `{action:"continue"}` | 观测钩子 |

工具数组元素：

~~~json
{
  "id": "echo",
  "description": "Echo through Extism",
  "input_schema": {"type": "object", "properties": {"text": {"type": "string"}}},
  "capabilities": []
}
~~~

## 14.3 宿主桥

所有宿主访问走一个导入函数：

~~~rust
#[host_fn]
extern "ExtismHost" {
    fn bsh_host(input: String) -> String;
}
~~~

请求 `{"op": "...", ...}`，响应 `{"ok": true, "value": ...}` 或
`{"ok": false, "error": "..."}`。

| op | 请求 | 成功值 | 能力 |
|---|---|---|---|
| `log` | `{message}` | null | 无 |
| `config` | `{}` | 配置 JSON 字符串 | 无 |
| `now_millis` | `{}` | Unix 毫秒 | 无 |
| `read_file` | `{path}` | `{text}` 或 `{base64}` | `workspace.read` |
| `write_file` | `{path, data}` | null | `workspace.write` |
| `fetch` | `{url, method?, body?}` | `{text}` 或 `{base64}` | `network.http` |
| `storage_get` | `{key}` | `{text/base64}` 或 null | `extension.storage` |
| `storage_set` | `{key, data}` | null | `extension.storage` |
| `storage_list` | `{}` | 键数组 | `extension.storage` |
| `read_secret` | `{name}` | 字符串 | `secrets.read` |
| `emit` | `{topic, payload}` | null | 主题需在 `event.publish` 声明（或 `tool.*`） |
| `subscribe` | `{topic}` | 订阅 id | 主题需在 `event.subscribe` 声明 |

桥的 `emit` / `subscribe` 按**声明**放行；`events.publish` / `events.subscribe`
这类能力在激活时按 `required` 检查，不在每次桥调用时重复检查。

细节：

- `read_file` / `write_file` 的路径相对工作区，且必须留在工作区内；
- `fetch` 只允许 http/https，15 秒超时，**只返回 body**，不返回状态码/头；
  每次交换都会产生一个 `http.exchange` 观测；
- `read_secret` 读的是环境变量 `BSH_EXT_SECRET_<NAME>`（大写）；
- 桥请求/响应体上限 8 MiB；
- 插件调用是串行的（一个插件互斥），取消会中断当前调用。

## 14.4 结果形状

`bsh_call_tool` 可以返回任意 JSON。要返回富内容，用 MCP 形状：

~~~json
{
  "content": [{"type": "text", "text": "done"}],
  "structuredContent": {"ok": true},
  "isError": false
}
~~~

## 14.5 构建

~~~bash
cd guest
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/<crate>.wasm ../plugin.wasm
~~~

`Cargo.toml` 依赖 `extism-pdk` 与 `serde_json`；`crate-type = ["cdylib"]`。

## 14.6 实验：Extism Echo

~~~bash
$BSH ext install examples/extensions/extism-echo --root /tmp/hello-root
$BSH ext mock examples/extensions/extism-echo echo '{"text":"hi"}' --root /tmp/hello-root
~~~

`mock` 会真正加载 wasm 并调用插件；它给所有能力发 `any` 授权，所以能验证
工具契约，但验证不了真实授权。

## 14.7 常见错误

| 现象 | 原因 | 处理 |
|---|---|---|
| `Extism plugin file is missing` | `entry` 路径错 | 对齐 `runtime.config.entry` |
| 工具列表为空 | 没导出 `bsh_tools`，也没声明工具文件 | 补 `tools/<id>.json` |
| 桥返回 `unknown host operation` | op 名拼错 | 对照上表 |
| `not declared in contributes.publishes` | 主题未声明 | 加 `event.publish` 贡献 |
| 调用后无响应 | 插件 panic / 死循环 | 看宿主日志与 wasm 调试输出 |
| 钩子从不触发 | 包未应用或未授权 | `why`、`grant hooks.observe` |

## 14.8 练习

1. 给插件加一个 `storage_get/set` 计数器；
2. 让 `bsh_command` 返回插入文本；
3. 写一个 `bsh_panel` 显示计数器；
4. 解释为什么 `fetch` 不返回状态码（提示：桥的最小契约）。

---

<a id="ch15"></a>
# 第 15 章 WASM component 运行时

**学习目标**：理解 WIT 契约的导入/导出；能构建一个 Component；知道 fuel/epoch/
内存限额怎样生效。

## 15.1 WIT 世界

契约在 `crates/bsh/wit/extension.wit`：

~~~text
world bsh-extension {
  import host;
  export extension;
}
~~~

导入（宿主提供，全部可能被拒绝）：

~~~text
log(level, message)
emit(event) -> result
subscribe(topic) -> result<u64>
unsubscribe(id)
read-file(path) -> result<list<u8>>
write-file(path, bytes) -> result
fetch(url, method, body) -> result<list<u8>>
storage-get/set/list
read-secret(name) -> result<string>
configuration() -> string
now-millis() -> u64
progress(value, total)
~~~

导出（宿主按顺序调用）：

~~~text
initialize(host-version, config) -> result
activate() -> result
tools() -> list<tool-definition>
call-tool(id, arguments, correlation-id) -> result<tool-result>
command(id, arguments) -> result<string>
panel(id) -> result<string>
on-event(event)
deactivate() -> result
shutdown() -> result
~~~

- `tool-definition` = `{id, description, input-schema}`（schema 是 JSON 字符串）；
- `tool-result` = `{content, is-error}`（content 是 JSON 字符串）；
- 失败用 `host-error`：`denied / not-found / limited / runtime / timeout`。

## 15.2 清单

~~~toml
[runtime]
kind = "wasm-component"

[runtime.config]
entry = "wasm/extension.wasm"
startup = "on-demand"

[capabilities]
required = ["agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"
~~~

工具描述符来自 `tools()` 导出，**不需要** `tools/echo.json`。但如果你声明了
command/panel，宿主仍会加载对应的声明式文件（面板需要一个初稿 JSON）。

## 15.3 构建

~~~bash
rustup target add wasm32-unknown-unknown
cd guest
cargo build --target wasm32-unknown-unknown --release
wasm-tools component new \
  target/wasm32-unknown-unknown/release/<crate>.wasm \
  -o ../wasm/extension.wasm
~~~

guest 用 `wit-bindgen` 生成绑定，WIT 文件通常复制一份到 `guest/wit/`。

## 15.4 限额

| 限制 | 实现 |
|---|---|
| 线性内存 | StoreLimits（`max_memory_mb`） |
| CPU | fuel（每次调用扣减） |
| 墙钟 | epoch 中断（10 ms 一跳） |
| 输出 | 宿主包装器上限 8 MiB + `max_output_bytes` |
| 调用串行 | 一个实例一把锁 |

工具描述符里的 `capabilities` 是空列表：WASM 的授权发生在**导入函数**里
（读文件时查 `workspace.read`，fetch 时查 `network.http`），而不是工具描述符。
这与 Extism 不同，写文档时要说清。

## 15.5 实验

~~~bash
$BSH ext validate examples/extensions/wasm-echo
$BSH ext install examples/extensions/wasm-echo --root /tmp/hello-root
$BSH ext info com.example.wasm-echo --root /tmp/hello-root
~~~

若环境能编译 component（或直接用仓库里的 `wasm/extension.wasm`），`mock` 会调用
`call-tool`。

## 15.6 常见错误

| 现象 | 原因 | 处理 |
|---|---|---|
| 编译失败 | 没加 component target / wit 不匹配 | 用 `wasm-tools component new` |
| `initialize` 失败 | 版本不兼容 | 返回 `ok`，检查 host-version |
| `tools()` 为空 | 忘了导出工具 | 在 `tools()` 返回定义 |
| 调用 trap | panic / 越界 | 修 guest，看宿主日志 |
| `limited` | fuel/内存耗尽 | 提高资源或优化 |

## 15.7 练习

1. 给 `tools()` 加第二个工具；
2. 用 `read-file` 读工作区文件，观察没有 `workspace.read` 时的错误；
3. 说明为什么 WASM 不在工具描述符上声明能力。

---

<a id="ch16"></a>
# 第 16 章 Cordis 运行时

**学习目标**：用 `apply(ctx, config)` 注册工具/命令/技能；理解 Node 桥的边界。

## 16.1 清单

~~~toml
[runtime]
kind = "cordis"

[runtime.config]
entry = "index.js"
startup = "on-demand"

[capabilities]
required = ["process.execute", "agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"

[[contribution]]
kind = "command"
id = "hello"

[[contribution]]
kind = "skill"
id = "greet"
~~~

- 需要一个 Node 解释器（宿主查找 `node`/`bun`/`deno`）；
- `entry` 是包相对 JS；
- 需要 `process.execute`（桥是子进程）与 `agent.tools.register`。

## 16.2 入口

~~~js
module.exports = {
  apply(ctx, config) {
    ctx.tools.register({
      name: "echo",
      description: "Echo a value through the Cordis runtime.",
      parameters: {
        type: "object",
        properties: { value: { type: "string" } },
        required: ["value"],
      },
      output: { schema: {}, render: () => null },
      async execute(args) {
        const terminal = await ctx.terminal.run("printf bridge-ok");
        return { echoed: args.value, terminal: terminal.stdout };
      },
    });
    ctx.commands.register({
      name: "hello",
      description: "Say hello from the Cordis runtime.",
      handler: () => ({ kind: "success", text: "hello from cordis" }),
    });
    ctx.skills.register({
      name: "greet",
      description: "Greet the reader from the Cordis runtime.",
      content: "# Greet\n\nSay hello from the Cordis runtime.",
    });
  },
};
~~~

- `ctx.tools.register`：注册工具（名称、描述、参数 schema、execute）；
- `ctx.commands.register`：注册命令；
- `ctx.skills.register`：注册技能（内容写进 SKILL.md）；
- `ctx.terminal.run`：通过宿主终端执行命令（受 `process.execute` 约束）。

## 16.3 桥的边界

~~~text
BSH 宿主 ──子进程──▶ Node bridge ──▶ 包 index.js
   ▲                    │
   └──── 注册/调用 ─────┘
~~~

桥是子进程，包**没有**宿主进程内访问权。注册类型有 tool / command / skill /
hook 四种；工具调用按注册 id 代理回桥。

## 16.4 限额与失败

- 启动失败通常是"找不到解释器"或入口报错；
- 桥调用是同步请求/响应，长任务会占住一次调用；
- 终端调用需要终端服务；没有服务时 `ctx.terminal.run` 失败；
- 钩子注册在桥层存在，但本构建的清单校验只允许 Extism 声明 `hook` 贡献，
  所以不要把 Cordis 当作钩子运行时的替代。

## 16.5 实验

~~~bash
$BSH ext validate examples/extensions/cordis-echo
$BSH ext install examples/extensions/cordis-echo --root /tmp/hello-root
$BSH ext info com.example.cordis-echo --root /tmp/hello-root
~~~

如果环境有 Node，`info` 会出现 `cordis-echo__echo`。

## 16.6 练习

1. 在 `execute` 里读 `config` 并影响返回值；
2. 注册第二个命令；
3. 解释为什么 Cordis 需要 `process.execute`。

---

<a id="ch17"></a>
# 第 17 章 能力与授权

**学习目标**：读懂 21 项能力与风险；理解"声明—授权—每次调用检查"三段式；会用
grant/revoke 与审计。

## 17.1 三段式

~~~text
1. 声明（manifest）      required = ["workspace.write"]
2. 授权（policy）        grant / 默认规则 / 用户决定
3. 调用（per call）      每次调用按参数解析作用域，再问一次
~~~

三段缺一不可：

- 没声明：未知能力名在验证阶段被拒绝；激活所需能力必须出现在 required；
- 没授权：激活失败（required）或调用被拒；
- 调用时作用域不匹配：即使有授权也可能被拒（例如授权了 A 目录，却写 B 目录）。

## 17.2 能力词表（21 项）

| 能力 | 风险 | 含义 |
|---|---|---|
| `workspace.read` | low | 读工作区内文件 |
| `workspace.write` | high | 写/创建/删除工作区内文件 |
| `network.http` | high | 发起出站 HTTP |
| `process.execute` | critical | 启动并管理外部进程 |
| `computer.use` | critical | 读屏幕/驱动键鼠 |
| `secrets.read` | critical | 从 vault 读命名秘密 |
| `extension.storage` | low | 使用扩展私有存储命名空间 |
| `agent.tools.register` | low | 注册工具 |
| `agent.context.provide` | low | 提供作用域上下文 |
| `agent.prompts.provide` | low | 提供提示片段 |
| `agent.skills.provide` | medium | 提供 SKILL.md 技能 |
| `agent.models.provide` | medium | 注册模型/提供方适配器 |
| `agent.agents.provide` | medium | 注册 agent 角色/策略 |
| `agent.workflows.provide` | medium | 注册工作流 |
| `events.publish` | low | 发布事件 |
| `events.subscribe` | low | 订阅事件 |
| `ui.panels` | low | 贡献面板 |
| `ui.commands` | low | 贡献命令/快捷键 |
| `ui.theme` | low | 贡献主题资源 |
| `hooks.observe` | high | 接收观测钩子批次 |
| `agent.intercept` | high | 注册事件拦截器；钩子 intercept 仍未实现 |

历史别名：`terminal.execute` 等价于 `process.execute`。实时列表以
`bsh ext capabilities` 为准。

## 17.3 默认策略

| 信任级别 | 默认放行 |
|---|---|
| `builtin` | 全部（作用域 any） |
| `verified` | 风险 ≤ medium，按默认作用域 |
| `community` / `local` / `unknown` | 低风险 + `agent.skills.provide`，按默认作用域 |

其余需要显式授权：

~~~bash
$BSH ext grant com.example.hello workspace.write --root /tmp/root --scope path:/tmp/ws --note "edits"
$BSH ext revoke com.example.hello workspace.write --root /tmp/root
~~~

`revoke` 之后宿主会重建视图；正在进行的调用不会被回滚。

## 17.4 作用域语法

| 作用域 | 含义 |
|---|---|
| `any` | 不受限 |
| `workspace` | 当前工作区 |
| `path:/abs/or/rel` | 具体路径（写时做包含检查） |
| `domain:api.example.com` | 主机名 |
| `domain:api.example.com:POST` | 主机名 + 方法 |
| `program:python3` | 程序 |
| `program:python3:server.py` | 程序 + 参数 |
| `secret:OPENAI_KEY` | 秘密名 |

作用域从调用参数的 `path`、`url`、`secret` 提取；参数名不在约定内时，作用域
可能退化为 `any` 或 `workspace`，导致"授权了却仍被拒"或"范围过大"。这是
命名约定的重要性（第 5.7 节）。

## 17.5 审计

每次授权决策写一行 JSON 到审计日志：

~~~json
{"timestamp":1793702405,"extension":"com.example.hello",
 "capability":"agent.tools.register","allowed":true,
 "scope":"any","trust":"local","reason":null}
~~~

~~~bash
$BSH ext logs com.example.hello --root /tmp/root
~~~

能读三件事：谁、什么能力、作用域与结果。拒绝时 `reason` 给出人话原因。

## 17.6 注册 ≠ 授权

注册工具是 `agent.tools.register`（低风险），但工具体里用文件/网络/进程仍需
相应能力。一个包可以：

- 注册十个工具（低风险）；
- 其中一个申请 `workspace.write`（高风险）；
- 用户只授权那一个工具需要的能力。

## 17.7 练习

1. `agent.tools.register` 是低风险，为什么仍然需要声明？
2. 授权 `network.http` 为 `domain:api.example.com`，调用
   `https://api.example.com/v1` 会通过吗？`https://evil.example.com` 呢？
3. 为什么 `hooks.observe` 是 high 而不是 low？

---

<a id="ch18"></a>
# 第 18 章 最小权限与拒绝诊断

**学习目标**：为一个真实包设计最小权限；读懂拒绝信息；避免"mock 通过、线上被拒"。

## 18.1 最小权限配方

1. 先只声明完成功能**必需**的能力；
2. 把高风险能力拆到少数工具（描述符级 `capabilities`）；
3. 用最窄作用域授权（路径、域名+方法、程序）；
4. 每加一个能力，问：没有它功能会坏在哪一步？
5. 定期看审计：有哪些授权从未使用？

## 18.2 一个完整例子

~~~toml
[capabilities]
required = [
  "agent.tools.register",
  "extension.storage",
]
~~~

~~~json
{
  "description": "保存一条工作区笔记",
  "input_schema": {
    "type": "object",
    "properties": { "path": {"type": "string"}, "text": {"type": "string"} },
    "required": ["path", "text"]
  },
  "capabilities": ["workspace.write"]
}
~~~

- 包级只要低风险能力；
- 写入能力只挂在一个工具上；
- 授权时用 `--scope path:/tmp/ws`。

## 18.3 拒绝长什么样

安装/激活阶段：

~~~text
com.example.extism-hook: not applied: extension com.example.extism-hook requires
capability hooks.observe, which policy denied: hooks.observe requires an explicit
grant for a local extension (run 'bsh ext grant com.example.extism-hook hooks.observe')
~~~

调用阶段（模型看到的工具错误；下面是示意，实际文本以错误为准）：

~~~text
extension com.example.hello is not authorized for workspace.write:
the path /etc/passwd is outside the granted scope
~~~

阶段不同，处理不同：

| 阶段 | 处理 |
|---|---|
| 激活失败 | `bsh ext grant ...` 后重试 `reload` / `retry` |
| 调用失败 | 检查作用域与参数名；必要时重授权 |
| mock 通过但真实失败 | mock 不查策略；按真实授权补 |

## 18.4 实验：观察一次拒绝

1. 在 `/tmp/hello-lab` 加一个声明 `workspace.write` 的工具；
2. 安装后看 `info`：`capability` 行会显示 `workspace.write [high] denied`；
3. 调用它（或运行 agent），读错误；
4. 授权：

   ~~~bash
   $BSH ext grant com.example.hello workspace.write --root /tmp/hello-root \
     --scope path:/tmp/hello-root --note "lab"
   ~~~

5. 再调用，读 audit：

   ~~~bash
   $BSH ext logs com.example.hello --root /tmp/hello-root
   ~~~

## 18.5 高风险能力的审查问题

| 能力 | 审查问题 |
|---|---|
| `workspace.write` | 能否只写一个子目录？是否可逆？ |
| `network.http` | 目标域名固定吗？会不会把用户数据发出去？ |
| `process.execute` | 程序与参数固定吗？会不会被输入注入？ |
| `secrets.read` | 真的需要读秘密，还是宿主可以代持？ |
| `hooks.observe` | 需要哪些点？会不会看到 prompt？ |
| `agent.intercept` | 只放行事件拦截器注册；钩子 intercept 未实现，声明它仍会扩大风险面 |

## 18.6 练习

1. 给一个"只读 Git 状态"的包设计能力与作用域；
2. 一个包需要访问两个域名，怎样写授权更安全？
3. 为什么 `hooks.observe` 的授权没有更细的作用域？你能提出什么设计？

---

<a id="ch19"></a>
# 第 19 章 生命周期、状态与激活

**学习目标**：读懂 `list`/`info`/`why` 的状态列；会用 enable/disable/retry/
quarantine/rollback/versions 处理常见运维场景。

## 19.1 状态机

~~~text
available -> installed -> resolved -> applied -> active
                    \-> failed / quarantined
~~~

生命周期枚举更细：discovered、validating、resolved、installing、installed、
loading、starting、active、suspending、suspended、stopping、unloaded、failed。
你在 `list` 里最常见的是 `active`、`resolved`、`failed`。

## 19.2 四个容易混淆的词

| 词 | 含义 |
|---|---|
| `enabled` | 用户开关（settings/state） |
| `configured` | 配置可用（有默认值或用户文件） |
| `applied` | 管理器已完成 plan + apply，运行时与贡献租约都在 |
| `active` | 运行时已启动、可接受调用 |

`list` 的 `STATE` 是生命周期状态；`APPLIED` 是布尔。一个包可以
`resolved` 但未应用（缺能力/被禁用），也可以 `active` 但未 applied（极少见，
通常是诊断窗口）。

## 19.3 常用命令与真实输出

~~~bash
$BSH ext disable com.example.hello --root /tmp/root
$BSH ext list --root /tmp/root
$BSH ext enable com.example.hello --root /tmp/root
$BSH ext versions com.example.hello --root /tmp/root
$BSH ext rollback com.example.hello --root /tmp/root
$BSH ext retry com.example.hello --root /tmp/root
$BSH ext quarantine com.example.hello --reason "policy review" --root /tmp/root
$BSH ext uninstall com.example.hello --root /tmp/root
$BSH ext uninstall com.example.hello --purge-data --root /tmp/root
~~~

禁用后的 `list`：

~~~text
com.example.hello                  0.1.0      resolved     local      no       hello__echo
~~~

`versions` 输出：

~~~text
0.1.0 (active)
~~~

`uninstall` 默认保留数据；`--purge-data` 才删除 `extension-data/<id>`。

## 19.4 plan、graph、why、effects

| 命令 | 回答什么 |
|---|---|
| `plan` | 管理器会按什么顺序激活，哪些包在计划里 |
| `graph` | 依赖与服务边，谁 provides/requires |
| `why <id>` | 这个包为什么被/没被应用 |
| `effects <id>` | 声明的效果面（工具、命令、事件、钩子等） |

`effects` 输出示例：

~~~text
tool:echo
command:refresh
~~~

`plan --json`：

~~~json
{
  "order": ["com.example.extism-hook", "com.example.process-echo"]
}
~~~

## 19.5 失败处理路径

~~~text
装完 → list 显示 failed
  → why <id> 读原因
     ├─ 缺能力：grant，然后 retry / reload
     ├─ 依赖/服务不满足：先装依赖
     ├─ 运行时启动失败：看 logs，修 entry/代码
     └─ 被隔离：quarantine 后走审计流程
~~~

`retry` 会清除持久化失败并重新加载；`reload` 只重建视图，不改变授权。

## 19.6 升级与回滚

~~~bash
$BSH ext update /path/to/new-version --root /tmp/root
$BSH ext versions com.example.hello --root /tmp/root
$BSH ext rollback com.example.hello --root /tmp/root
$BSH ext reproduce /path/to/lock.toml --root /tmp/root
~~~

`reproduce` 会校验已安装版本的 hash 与锁文件一致，不一致直接拒绝。

## 19.7 练习

1. 用 `disable` 后，工具还会出现在模型列表里吗？
2. `quarantine` 与 `uninstall` 的区别是什么？数据还在吗？
3. 一个包 `resolved` 但 `applied no`，你会先跑哪三条命令？

---

<a id="ch20"></a>
# 第 20 章 存储、哈希、签名与分发

**学习目标**：知道文件放在哪；理解包哈希与签名覆盖什么；能打包、分发、校验。

## 20.1 托管存储布局

~~~text
<root>/
  installed/<id>/<version>/      不可变代码与资源
  extension-data/<id>/           扩展业务数据（默认 uninstall 保留）
  extension-state/               state.toml / registry.toml / lock.toml /
                                 grants.toml / trust.toml
  logs/extensions/audit.jsonl   审计
  quarantine/                    隔离的代码
~~~

`<root>` 默认是 `~/.bos/bsh/extensions`；`BSH_EXTENSION_ROOT` 可覆盖；
`--root` 临时覆盖。

## 20.2 哈希

`package_hash` 的规则：

- 按相对路径排序，逐个文件做 SHA-256；
- 忽略 `.git`、`target`、`node_modules`、`.jj`；
- 不含目录元数据、mtime、权限位；
- 结果是一个十六进制摘要，用于安装记录、锁文件、签名与 `doctor`。

所以"同样的文件内容"得到同样的 hash；改一个字节就变。

## 20.3 签名

签名覆盖一个规范消息：

~~~text
bsh-package-v1
<extension id>
<version>
<package hash>
~~~

`package.sig` 是 JSON：

~~~json
{
  "schema_version": "1.0",
  "publisher": "acme",
  "public_key": "<base64>",
  "algorithm": "ed25519",
  "signature": "<base64>"
}
~~~

流程：

~~~bash
$BSH ext keygen acme --out /secure/acme.key
$BSH ext sign /path/to/pkg --publisher acme --key /secure/acme.key
$BSH ext trust acme <base64-public-key> --root /tmp/root
$BSH ext verify /path/to/pkg --root /tmp/root
~~~

- `keygen` 的 `--out` 是**文件路径**（写私钥种子），不是目录；
- `trust` 把公钥写入 `extension-state/trust.toml`；
- 未签名包在策略允许时仍可安装；
- 签名证明"谁发的、内容没变"，不证明行为安全。

> **现状提醒**：3.0.2 的 `sign` / `verify` 仍用 1.x 清单解析器，对 2.0 包会报
> `unknown field host`。机制与文件格式如上；CLI 修复见附录 I。

## 20.4 构建发布物

~~~bash
$BSH ext build /path/to/pkg --out /tmp/dist
~~~

~~~text
built /tmp/dist/com.example.hello-0.1.0.tar.gz
hash:  60e15cf5...
~~~

`build` 先校验再打包；包内保留目录结构。发布前建议先在本机 `install --root`
一次，确认激活与贡献都正常。

## 20.5 市场与私有 registry

官方市场是 **Git 仓库**，不是服务：

~~~text
bsh_extensions/
  index.json
  packages/<id>/<version>/...
~~~

~~~bash
$BSH ext market                 # 列出条目
$BSH ext market install <id>    # 装最新兼容版本
$BSH ext registry add private https://example.com/extensions
$BSH ext registry list
$BSH ext registry remove private
$BSH ext search <query> --registry private
~~~

市场需要网络；离线环境用 `install <archive|path>`。

## 20.6 信任级别

| 级别 | 来源 | 默认策略 |
|---|---|---|
| `builtin` | 链入宿主 | 全放行 |
| `verified` | 已验证发布者 | 中低风险放行 |
| `community` | 公共来源 | 仅低风险 |
| `local` | 开发目录 | 仅低风险 |
| `unknown` | 来源不明 | 仅低风险 |

## 20.7 练习

1. 修改包内一个 README，hash 会变吗？改 mtime 呢？
2. 签名能防止什么？不能防止什么？
3. 为什么 `uninstall` 默认保留数据？什么时候该用 `--purge-data`？

---

<a id="ch21"></a>
# 第 21 章 依赖与服务

**学习目标**：声明依赖与服务；理解解析顺序、循环检测与"发现不等于授权"。

## 21.1 依赖

~~~toml
[dependencies]
"com.example.base" = "^1.0"
~~~

- 键是扩展 id，值是 semver 要求；
- 循环与不满足在激活前拒绝；
- 安装记录与锁文件记录解析结果；`reproduce` 按锁恢复。

## 21.2 服务

~~~toml
[services]
provides = []
requires = []

[[services.provides]]
name = "com.example.search"
contract = "com.example.search-api"
version = "1.0.0"
scope = "workspace"
capabilities = []

[[services.requires]]
name = "com.example.config"
contract = "com.example.config-api"
version = "^1.0"
scope = "extension"
optional = true
~~~

| 字段 | 含义 |
|---|---|
| `name` | 提供方本地名 |
| `contract` | 服务身份（真正的键） |
| `version` | 提供方版本 / 消费方要求 |
| `scope` | host / workspace / session / extension |
| `capabilities` | 消费方仍需持有的能力 |
| `optional` | 不满足时是否阻止激活 |

规则：

- 服务身份是 `contract`，不是 `name`；
- 解析器同时看包 DAG 与 provider/consumer 边；
- 必需的、无法满足的服务会阻止激活；`optional = true` 则继续；
- **服务发现不是授权**：消费方仍要持有提供方声明的能力。

## 21.3 顺序

`bsh ext plan` 给出管理器计划；依赖与服务边决定先后。`graph` 显示
`PROVIDES -> REQUIRES` 列。

## 21.4 实验

1. 造两个包 A、B，A 依赖 B，观察 `plan` 顺序；
2. 制造循环依赖，读错误；
3. 给 A 加一个必需服务，但没人提供，观察 `why` 的原因。

## 21.5 练习

1. 为什么用 contract 而不是 name 作为服务身份？
2. 一个 optional 服务缺失时，消费方应该如何降级？
3. 服务提供方声明了 `network.http`，消费方需要自己再申请吗？

---

<a id="ch22"></a>
# 第 22 章 观测钩子：原理与契约

**学习目标**：理解 observe 钩子的队列/批处理/脱敏契约；知道五个点各自给什么；
能判断哪些数据"拿不到"。

## 22.1 与事件的区别（再强调一次）

事件是事后广播；钩子是**在接缝上把数据交给声明了该点的包**。本构建只实现
**observe**：它不阻塞调用方，也不能否决/改写。设计中的 intercept（deny /
replace / timeout）没有落地，清单里也没有字段能开启它。

## 22.2 五个点

| 点 | 产生位置 | 载荷主题 | 典型用途 |
|---|---|---|---|
| `model.request` | agent `BeforeLlmCall` | model、request_id、prompt 形状 | 采集、提示结构指标 |
| `model.response` | agent `AfterLlmCall` | usage、响应类型/文本、request_id | 成本、延迟、答案形状 |
| `tool.before` | agent `BeforeToolCall` | 工具名、call_id、参数 | 审计、redaction 审查 |
| `tool.after` | agent `AfterToolCall` | 工具名、call_id、结果 | 审计、错误分析 |
| `http.exchange` | 内置 LLM vendor 的 HTTP tap；Extism/WASM `fetch` | 状态、字节数、TTFB、request_id | 线级形状采集 |

## 22.3 投递链路

~~~text
接缝 observe(point, payload)        立即返回
   │
   ▼
全局有界队列（1024）                 满则丢弃
   │
   ▼
专用线程（50 ms 轮询）               每次取 ≤64 条
   │  按 point 分组
   ▼
对每个已应用的 Extism 目标          一次调用一个 point 的批次
   │
   ▼
bsh_hook({"point": ..., "batch": [...]})
   │
   ▼
返回 {"action":"continue"}           observe 忽略 action
~~~

保证：

- **不阻塞接缝**：只入队；
- **批处理**：一个 point 的批次一次调用；
- **有界**：队列 1024，批次 ≤64；
- **脱敏**：`sk-`、`ghp_`、`Bearer ` 后的值被替换为 `[redacted]`；
- **单字段上限**：每个字段约 8K 字符（redaction 后截断）；
- **隔离**：一个包失败不影响其他包，失败写 warn 日志；
- **无宿主锁**：投递给 guest 时不持有宿主锁。

不保证：

- **送达**：队列满会丢；guest 慢会让后续事件排队；
- **跨点顺序**：不同 point 的批次没有全局顺序；
- **恰好一次**：重试/回退会产生同一 `request_id` 的多次观测；
- **正文**：没有 headers、没有 body，也没有 SSE 逐帧间隔；
- **进程/MCP 的 HTTP**：子进程自己发请求，宿主不在路径上。

## 22.4 一批长什么样

~~~json
{
  "point": "model.request",
  "batch": [
    {
      "agent": "assistant",
      "data": {
        "model": "gpt-4o",
        "request_id": "req-...",
        "request": "{\"request_id\":\"req-...\",\"model\":\"gpt-4o\",\"api_mode\":\"chat\",\"temperature\":0.7,\"max_tokens\":null,\"prompt_bytes\":1834,\"messages\":[{\"role\":\"system\",\"bytes\":1200,\"preview\":\"You are...\"}]}"
      }
    }
  ]
}
~~~

注意模型/工具事件的 `data` 每个值都是**字符串**（数字/布尔也序列化成字符串），
这是 redaction 层的统一处理；`http.exchange` 则直接给出带类型的字段。

## 22.5 model.request 的内容

`data.request` 是 JSON 字符串，字段：

| 字段 | 说明 |
|---|---|
| `request_id` | 本次请求 id |
| `model` | 模型名 |
| `api_mode` | `chat` 等 |
| `temperature` / `max_tokens` | 采样参数，可能为 null |
| `prompt_bytes` | 输入文本字节数 |
| `messages` | 最后 ≤48 条：`{role, bytes, preview}` |

`preview` 每条最多 512 字符；**没有完整 prompt**。这是刻意的：钩子是采集
接缝，不是 prompt dump。

## 22.6 model.response 的内容

完整响应：

~~~json
{
  "agent": "assistant",
  "data": {
    "request_id": "req-...",
    "response_type": "react",
    "response": "{\"response_type\":\"react\",\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":40,\"total_tokens\":160}}"
  }
}
~~~

流式步骤（每个 step 一次，不是每 token）：

~~~json
{
  "agent": "assistant",
  "data": {
    "request_id": "req-...",
    "response_type": "stream",
    "response_bytes": "320",
    "response_text": "...",
    "had_tool_call": "false"
  }
}
~~~

## 22.7 tool.before / tool.after

~~~json
{"agent":"assistant","data":{
  "tool_name":"hello__echo","call_id":"call-1","tool_args":"{\"text\":\"hi\"}"
}}
~~~

~~~json
{"agent":"assistant","data":{
  "tool_name":"hello__echo","call_id":"call-1","tool_result":"{\"text\":\"hi\"}"
}}
~~~

`tool_args` / `tool_result` 是字符串，已脱敏、已截断。

## 22.8 http.exchange

~~~json
{
  "source": "com.example.extism-echo",
  "request_id": "req-...",
  "method": "POST",
  "host": "api.deepseek.com",
  "path": "/chat/completions",
  "status": 200,
  "request_bytes": 18402,
  "response_bytes": 91234,
  "content_type": "application/json",
  "ttfb_ms": 430,
  "total_ms": 6110,
  "error": null
}
~~~

- `path` 去掉 query/fragment；
- 没有 header 名/值，没有 body；
- `request_id` 由 vendor tap 关联到 `model.request`；Extism/WASM 的 fetch
  没有 request_id 时为 null；
- 失败时 `status` 为 null，`error` 有文本。

## 22.9 谁能收到

只有**已应用的 Extism 包**，且声明了该点。未应用（缺授权、manual、依赖失败）
的包不在目标列表里，收不到——这解释了"装好了但计数为零"。

## 22.10 练习

1. 为什么 observe 不返回 deny？如果要 deny，链路上哪里需要改？
2. 一批 200 个 chunk 会变成几次 guest 调用？
3. 一个包声明了 `model.request` 但没导出 `bsh_hook`，会发生什么？

---

<a id="ch23"></a>
# 第 23 章 声明、授权并实现一个观察器

**学习目标**：从零构建 `extism-hook` 风格的观察器：清单、guest、构建、安装、
授权、验证。

## 23.1 清单

~~~toml
schema_version = "2.0"

[extension]
id = "com.example.extism-hook"
name = "Extism Hook Observer"
version = "1.0.0"
description = "Consumes BSH observe hooks and counts them per point"

[compatibility]
host = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "extism"

[runtime.config]
entry = "plugin.wasm"
startup = "on-demand"

[capabilities]
required = [
  "hooks.observe",
  "extension.storage",
  "agent.tools.register",
  "ui.panels",
]

[[contribution]]
kind = "tool"
id = "stats"

[[contribution]]
kind = "panel"
id = "inspector"

[[contribution]]
kind = "hook"
id = "model.request"

[[contribution]]
kind = "hook"
id = "model.response"

[[contribution]]
kind = "hook"
id = "http.exchange"
~~~

校验规则：

1. 点必须是五个之一；
2. 不能重复；
3. 最多 32 个；
4. 本构建只允许 `extism`；
5. `hooks.observe` 必须在 `required`。

> 仓库里的 `examples/extensions/extism-hook/README.md` 仍用 1.x 的
> `[contributes]` 写法；以 `extension.toml` 的 2.0 写法为准。这是文档滞后，
> 不是代码支持 1.x。

## 23.2 guest

~~~rust
use extism_pdk::*;
use serde_json::{json, Value};

#[host_fn]
extern "ExtismHost" {
    fn bsh_host(input: String) -> String;
}

fn host(request: Value) -> Option<Value> {
    let raw = unsafe { bsh_host(request.to_string()) }.ok()?;
    let reply: Value = serde_json::from_str(&raw).ok()?;
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    Some(reply.get("value").cloned().unwrap_or(Value::Null))
}

fn read_counts() -> Value {
    host(json!({"op": "storage_get", "key": "counts"}))
        .and_then(|value| value.get("text").cloned())
        .and_then(|text| text.as_str().and_then(|text| serde_json::from_str(text).ok()))
        .unwrap_or_else(|| json!({}))
}

fn write_counts(counts: &Value) -> bool {
    host(json!({
        "op": "storage_set",
        "key": "counts",
        "data": counts.to_string(),
    }))
    .is_some()
}

#[plugin_fn]
pub fn bsh_hook(input: String) -> FnResult<String> {
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let point = payload
        .get("point")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let count = payload
        .get("batch")
        .and_then(Value::as_array)
        .map(|batch| batch.len() as u64)
        .unwrap_or(1);
    let mut counts = read_counts();
    let Some(object) = counts.as_object_mut() else {
        return Ok(json!({"action": "continue"}).to_string());
    };
    let current = object.get(&point).and_then(Value::as_u64).unwrap_or(0);
    object.insert(point, json!(current.saturating_add(count)));
    if !write_counts(&counts) {
        return Err(Error::msg("storage_set failed").into());
    }
    Ok(json!({"action": "continue"}).to_string())
}
~~~

再导出 `bsh_tools`（`stats`）、`bsh_call_tool`、`bsh_panel`。完整文件在
`examples/extensions/extism-hook/guest/src/lib.rs`。

## 23.3 构建

~~~bash
cd guest
cargo build --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/bsh_extism_hook.wasm ../plugin.wasm
~~~

## 23.4 安装与授权

~~~bash
$BSH ext install examples/extensions/extism-hook --root /tmp/root
$BSH ext grant com.example.extism-hook hooks.observe --scope any --root /tmp/root
$BSH ext grant com.example.extism-hook extension.storage --scope any --root /tmp/root
$BSH ext grant com.example.extism-hook agent.tools.register --scope any --root /tmp/root
$BSH ext grant com.example.extism-hook ui.panels --scope any --root /tmp/root
$BSH ext why com.example.extism-hook --root /tmp/root
~~~

未授权时 `why` 的真实输出：

~~~text
com.example.extism-hook: not applied: extension com.example.extism-hook requires
capability hooks.observe, which policy denied: hooks.observe requires an explicit
grant for a local extension (run 'bsh ext grant com.example.extism-hook hooks.observe')
~~~

授权后，`list` 应显示 `active/applied yes`。

## 23.5 运行

在 BSH 里开启一个 agent 会话，发一轮消息；然后：

- 打开 **Observe hooks** 面板（`inspector`），或
- 调用 `stats` 工具（若模型可见），或
- 用 CLI 看 metrics。

Extism 的 `fetch` 也会产生 `http.exchange`；如果没有 fetch，只有 model/tool
点会有计数。

## 23.6 不用 agent 验证

~~~bash
$BSH ext mock examples/extensions/extism-hook stats '{}' --root /tmp/root
~~~

`mock` 不跑 agent，所以计数为零是**正确**结果；它证明运行时契约与面板契约，
不证明钩子投递。

## 23.7 本章检查清单

- [ ] 五个点都拼写正确；
- [ ] `hooks.observe` 在 required；
- [ ] 只声明真正需要的点（少即是多）；
- [ ] `bsh_hook` 返回合法 JSON；
- [ ] 存储键有命名空间；
- [ ] 授权后 `why` 显示 applied；
- [ ] 用一轮真实对话验证，而不是只跑 mock。

---

<a id="ch24"></a>
# 第 24 章 采集与分析食谱

**学习目标**：用现有载荷做延迟、成本、错误、工具审计；知道隐私边界。

## 24.1 三路关联

`model.request`、`model.response`、`http.exchange` 共享 `request_id`
（Extism 自己的 fetch 没有 request_id）。把三者按 id 拼起来，就得到一次调用的
语义 + 传输视图。

## 24.2 延迟

- 首字节：`http.exchange.ttfb_ms`；
- 总时长：`http.exchange.total_ms` 或 response 的时间差；
- token 速率：`completion_tokens / ((total_ms - ttfb_ms)/1000)`。

没有逐帧间隔；不要声称能算 `max_gap_ms`。

## 24.3 成本

- 从 `model.response.response` 解析 `usage`；
- 维护一张价格表（模型 → 每百万 token 价格）；
- 按 `request_id` 聚合到会话/回合。

## 24.4 提示结构

- `prompt_bytes` 与 `messages[].bytes` 看上下文增长；
- `messages[].preview` 只用于人读，不要把它当完整 prompt 存；
- 比较两次请求的 message 数量变化，找"工具结果把上下文翻倍"的问题。

## 24.5 工具审计

`tool.before` / `tool.after` 给出工具名、call_id、参数/结果字符串。可以做：

- 危险参数检测（路径越界、命令拼接）；
- 失败率（结果里是否含 `isError`）；
- 调用链（同一 call_id 的 before/after 配对）。

## 24.6 错误分类

- `http.exchange.error` 非空 → 传输/连接错误；
- `status` 4xx/5xx → 服务端错误；
- `model.response.response` 里的 usage 缺失 → 可能没拿到完整响应。

## 24.7 隐私与保留

- 载荷没有 body，但有 preview（≤512 字符）与参数/结果字符串；
- 仍然可能包含用户数据，存储前要脱敏/截断；
- 用 `extension.storage` 时自己实现保留策略（按天/按大小）；
- 不要把采集数据发到网络，除非你有 `network.http` 且用户明确同意；
- 读秘密用 `read_secret`，不要把秘密写进面板或日志。

## 24.8 配额

- 队列有界，采集器要快：每批只做 O(batch) 工作；
- 存储写入要合并（例如每批一次 `storage_set`）；
- 面板只读聚合结果，不在渲染时扫描全量历史。

## 24.9 练习

1. 设计一个"每次对话的 token/延迟"面板 schema；
2. 如何检测"同一请求被重试"？看哪些字段？
3. 写出你的保留策略（时间/条目/字节）。

---

<a id="ch25"></a>
# 第 25 章 钩子调试

**学习目标**：用最少命令定位"钩子不触发/计数为零/面板空白"。

## 25.1 五个问题，按顺序问

1. **包应用了吗？** `bsh ext why <id>`
2. **点声明对了吗？** `bsh ext validate`，错误字段以 `contributes.hooks` 开头
3. **`hooks.observe` 授权了吗？** `bsh ext info <id>` 的 capability 行
4. **guest 导出 `bsh_hook` 了吗？** 没有则每次投递都会 warn 失败
5. **真的跑了 agent 吗？** `mock` 不产生 model/tool 钩子

## 25.2 症状表

| 症状 | 原因 | 处理 |
|---|---|---|
| `not applied` | 缺 `hooks.observe` 授权 | grant 后 `retry` |
| 校验报 `contributes.hooks` | 点名错/重复/非 Extism/缺 required | 对照第 23.1 节 |
| 计数一直为 0 | 没跑 agent；或包未应用 | 跑一轮真实对话；`why` |
| 面板空白 | `bsh_panel` 返回非法文档 | 用 `schema_version` + `root` |
| 只有 http 计数 | 没触发模型/工具 | 检查 agent 是否真的调用 |
| 偶发丢事件 | 队列满/guest 慢 | 减少每批工作量 |

## 25.3 日志

~~~bash
$BSH ext logs com.example.extism-hook --root /tmp/root
~~~

审计日志记录授权决策；钩子投递失败以宿主 warn 日志出现（目标
`bsh::extension`）。如果日志里反复出现 `hook model.request failed`，先查导出名。

## 25.4 最小复现

- 只声明一个点（`model.request`）；
- guest 只返回 `{"action":"continue"}`，不做存储；
- 跑一轮对话；
- 确认能收到后再加逻辑。

这能区分"投递问题"与"guest 逻辑问题"。

## 25.5 练习

1. 造一个缺少 `bsh_hook` 的插件，观察日志；
2. 造一个返回非法 JSON 的 `bsh_hook`，观察行为；
3. 解释为什么 `mock` 不能验证钩子。

---

<a id="ch26"></a>
# 第 26 章 测试策略

**学习目标**：用四层测试覆盖一个扩展；知道每层能证明什么、不能证明什么。

## 26.1 四层

| 层 | 工具 | 证明 | 不证明 |
|---|---|---|---|
| 1. 清单 | `bsh ext validate` | 语法、字段、声明与文件一致 | 运行时行为 |
| 2. 运行时预检 | `bsh ext test` | 运行时可构造、入口存在、贡献可加载 | 真实授权、真实调用 |
| 3. 单工具 | `bsh ext mock` | 工具契约与结果形状 | 策略、事件、钩子、面板 |
| 4. 宿主/端到端 | `cargo test -p bsh --lib extension::` 等 | 安装、激活、调用、拒绝、租约 | 你的业务逻辑正确性 |

## 26.2 `test` 到底做什么

`test` 在 `validate` 之后按运行时做预检：

- `declarative`：构造运行时（所有文件与 implementation 必须齐）；
- `process`：构造运行时 + 加载贡献（会要求 `tools/<id>.json`）；
- `mcp`：构造 MCP 运行时（不启动 server）；
- `extism`：`entry` 文件存在 + 工具描述符可读 + 声明式贡献可加载；
- `wasm-component`：`entry` 文件存在 + 声明式贡献可加载；
- `cordis`：`entry` 存在 + 找到 Node 解释器 + 贡献可加载；
- `builtin`：**拒绝**——用户包不能声明 builtin。

## 26.3 `mock` 的用法

~~~bash
$BSH ext mock /tmp/lab <tool> '<json>' --root /tmp/root
~~~

- 给所有能力发 `any` 授权；
- 用临时目录做数据根；
- 配置是 `{"mock": true}`；
- 会真正加载 wasm / 启动进程 / 调用 MCP（环境允许时）；
- 不跑 agent、不投递钩子、不显示面板。

## 26.4 给扩展写宿主测试

如果你的扩展是仓库内示例或有 Rust 集成测试，参考 `crates/bsh/src/extension/e2e.rs`
的模式：

1. 建临时 root；
2. 写包文件；
3. 走 `ExtensionHost::open` → install/apply；
4. 断言工具列表、调用结果、拒绝；
5. 清理。

测试要覆盖**拒绝路径**：缺能力、错作用域、超时、坏 JSON、重复 id。一个只测
成功路径的扩展，第一次面对真实用户就会在拒绝路径上失败。

## 26.5 一致性矩阵

| 场景 | validate | test | mock | 宿主 |
|---|---:|---:|---:|---:|
| 缺工具文件 | 失败 | 失败 | 失败 | 失败 |
| 缺 implementation | 失败 | 失败 | 失败 | 失败 |
| 能力不足 | 通过 | 通过 | 通过 | 失败 |
| 作用域不匹配 | 通过 | 通过 | 通过 | 失败 |
| 输出超限 | 通过 | 通过 | 失败 | 失败 |
| 钩子未授权 | 通过 | 通过 | 通过 | 失败 |

这张表解释了为什么"本地全绿，装上就红"。

## 26.6 CI 配方

~~~bash
cargo fmt --all -- --check
cargo clippy --workspace --exclude jsbos --exclude nbos --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo test -p ferris-core --features tokio
cargo test -p ferris-runtime --features "wasm extism cordis"
cargo test -p ferris-sdk
cargo test -p bsh --lib extension::
scripts/ci/check-extension-gold-standard.sh
scripts/ci/check-ferris-independence.sh
~~~

基线数字会随提交变化；把"全绿"当作门槛，不要把某个具体条数写死。

## 26.7 测试数据与隔离

- 每个测试用独立临时目录（pid/timestamp 后缀）；
- 不要写用户的 `~/.bos/bsh`；
- 需要进程/网络时用本地 fixture server，不访问公网；
- wasm 构建产物（`plugin.wasm`）可以入库，但要在 README 写清重建命令。

## 26.8 练习

1. 为第 5 章的 `greet` 写四个用例：正常、缺参数、错误类型、超长输出。
2. 设计一个"能力拒绝"测试，断言错误文本包含能力名。
3. 为什么 `mock` 不能替代宿主测试？

---

<a id="ch27"></a>
# 第 27 章 调试手册

**学习目标**：把"不工作"拆成可执行的五步；按症状查表。

## 27.1 前五条命令

~~~bash
$BSH ext validate <path>                 # 包本身对不对
$BSH ext why <id> --root <root>          # 为什么没应用
$BSH ext doctor --root <root>            # store/哈希/依赖/策略
$BSH ext logs <id> --root <root>         # 授权与审计
$BSH ext info <id> --root <root>         # 状态、工具、能力决策
~~~

`doctor` 正常输出（真实）：

~~~text
host:      3.0.2 (extension API 1.0.0)
root:      /tmp/hello-root
extensions: 4
0 error(s), 0 warning(s)
~~~

## 27.2 症状 → 原因

| 症状 | 先查 | 常见原因 |
|---|---|---|
| 包 invalid | `validate` 的错误字段 | 未知字段、缺文件、id 语法 |
| 装了但没应用 | `why` | 缺高风险授权、依赖、manual |
| 工具不可见 | `info` `tools` / `list` TOOLS | 运行时未注册、资格名冲突 |
| 调用被拒 | `logs` | 能力/作用域、参数名不规范 |
| 调用超时 | `info`/清单 resources | 限额太小、运行时阻塞 |
| 面板空白 | `validate` + 运行时日志 | UI 校验失败、动态文档非法 |
| 钩子无计数 | `why` + guest 导出 | 未授权、未导出 `bsh_hook`、没跑 agent |
| 进程起不来 | 手工跑 server | 入口、权限、外层沙箱 |
| hash 不符 | `doctor` | 安装后被改动 |
| 升级后坏了 | `versions` / `rollback` | 新版本不兼容 |

## 27.3 读审计

~~~bash
$BSH ext logs com.example.hello --root /tmp/root | tail -5
~~~

每行是一个决策。找 `"allowed": false` 与 `"reason"`；对比 `"scope"` 与调用
参数。

## 27.4 空白面板的四个原因

1. 包没应用（没有面板 surface）；
2. `view` 校验失败（错误在 `validate` 里以 `ui.` 开头）；
3. 动态 `bsh_panel` 返回非对象/缺 `root`；
4. 图片 `src` 不是 `asset/...`（只这一种图片会被画）。

## 27.5 握手失败

先脱离宿主验证：

~~~bash
cd <package>
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"token":"abc"}}' \
  | <entry> <args>
~~~

能手工回出协议行，再怀疑宿主；不能，就是 server 或环境问题。

## 27.6 钩子调试

见第 25 章。记住：`mock` 永远显示零计数；要跑真实对话。

## 27.7 一条命令看全局

~~~bash
$BSH ext metrics --root /tmp/root
~~~

关注：`tool_calls`、`tool_failures`、`events_published/delivered/dropped`、
`failed`、`active_tasks`、`memory_limit_bytes`/`memory_used_bytes`。

## 27.8 练习

1. 把一个包的 `runtime.entry` 写错，按五步定位。
2. 制造一次作用域拒绝，从 `logs` 读出原因。
3. 写出你的"第一次求助时应附上的四条信息"。

---

<a id="ch28"></a>
# 第 28 章 CLI 任务手册

**学习目标**：按任务找到命令；知道全局选项与退出码；能写脚本。

## 28.1 全局选项

- `--root <path>`：覆盖托管根（也可用 `BSH_EXTENSION_ROOT`）；
- `--json`：支持的命令输出机器可读结果（`plan`、`graph`、`doctor` 等）；
- 两者可以放在子命令前或后，解析器会全局识别。

退出码：0 成功，1 被拒绝/失败，2 用法错误。

## 28.2 作者任务

| 任务 | 命令 |
|---|---|
| 新建 | `ext new <id> [--dir <path>] [--force]` |
| 校验 | `ext validate <path>` |
| 预检 | `ext test <path>` |
| 单工具 | `ext mock <path> [tool] [json]` |
| 哈希 | `ext hash <path>` |
| 打包 | `ext build|package <path> [--out <dir>]` |
| 迁移 | `ext migrate <path> [--write]` |

## 28.3 检查任务

| 任务 | 命令 |
|---|---|
| 列表 | `ext list` |
| 详情 | `ext info <id>` |
| 计划 | `ext plan [--json]` |
| 图 | `ext graph [--json]` |
| 为什么 | `ext why <id>` |
| 效果面 | `ext effects <id>` |
| 诊断 | `ext doctor [--json]` |
| 指标 | `ext metrics` |
| 日志 | `ext logs <id>` |
| 版本 | `ext versions <id>` |

## 28.4 安装与运维

| 任务 | 命令 |
|---|---|
| 安装 | `ext install <path|archive|git+url>` |
| 注册表安装 | `ext install --registry <name|url> <id>[@version]` |
| 更新 | `ext update <path>` |
| 卸载 | `ext uninstall <id> [--purge-data]` |
| 启用/禁用 | `ext enable|disable <id>` |
| 回滚 | `ext rollback <id>` |
| 重试 | `ext retry <id>` |
| 隔离 | `ext quarantine <id> [--reason <text>]` |
| 重载 | `ext reload <id>` |
| 锁定恢复 | `ext reproduce <lock>` |
| 挂载补丁 | `ext mount <file>` |

## 28.5 能力与发布

| 任务 | 命令 |
|---|---|
| 能力表 | `ext capabilities` |
| 授权 | `ext grant <id> <cap> [--scope s] [--note n]` |
| 撤销 | `ext revoke <id> <cap>` |
| 密钥 | `ext keygen <publisher> [--out <file>]` |
| 签名 | `ext sign <path> --publisher <name> --key <file-or-base64>` |
| 验证 | `ext verify <path>` |
| 信任 | `ext trust <publisher> <public-key>` / `untrust` / `publishers` |
| 市场 | `ext market [query]` / `market install <id>` |
| 注册表 | `ext registry add|list|remove` / `ext search` |

## 28.6 描述符任务

| 任务 | 命令 |
|---|---|
| 提供方 | `ext providers` |
| 语言 | `ext languages` |
| 集成 | `ext integrations` |
| 语言诊断 | `ext language-check <id> <lang> <path>` |
| 运行集成 | `ext integrate <id> <integration> [json]` |
| 配置 | `ext config <id> [set <json|file>|schema]` |

## 28.7 脚本化

~~~bash
set -euo pipefail
BSH=./target/debug/bsh
ROOT=/tmp/ci-root
$BSH ext validate examples/extensions/hello
$BSH ext test examples/extensions/hello
$BSH ext install examples/extensions/hello --root "$ROOT"
$BSH ext why com.example.hello --root "$ROOT" | grep -q applied
$BSH ext doctor --root "$ROOT" | grep -q '0 error(s)'
~~~

注意：

- 不要解析人类输出里的列宽；用 `--json` 或 `grep -q`；
- 不要用 `why` 的退出码判断是否 applied；读它的输出文本；
- `install` 会复制包，后续改源目录不会自动生效，要 `update`/`reload`。

## 28.8 练习

1. 写一个脚本：校验、安装、检查 applied、打印 tools。
2. 用 `--json` 从 `plan` 里取顺序。
3. 解释 `--root` 与 `BSH_EXTENSION_ROOT` 的优先级关系（查看源码或试一次）。

---

<a id="ch29"></a>
# 第 29 章 打包、签名、发布与市场

**学习目标**：产出可分发归档；理解签名与信任；按清单发布。

## 29.1 发布流程

~~~text
1. validate + test + mock
2. 在本机隔离 root 安装并跑一轮真实使用
3. 必要时签名
4. build 出 .tar.gz
5. 发布到 Git 市场 / 私有 registry / 直接分发路径
6. 记录 hash 与版本
~~~

## 29.2 构建

~~~bash
$BSH ext build examples/extensions/hello --out /tmp/dist
~~~

~~~text
built /tmp/dist/com.example.hello-1.0.0.tar.gz
hash:  60e15cf5...
~~~

归档保留目录结构；安装端解包后仍要能通过 `validate`。

## 29.3 签名（机制）

~~~bash
$BSH ext keygen acme --out /secure/acme.key
$BSH ext sign /path/to/pkg --publisher acme --key /secure/acme.key
$BSH ext trust acme <public-key> --root /tmp/root
$BSH ext verify /path/to/pkg --root /tmp/root
~~~

签名覆盖一条规范消息：`bsh-package-v1`、扩展 id、版本、包哈希，各占一行；
结果写入 `package.sig`。

> **现状**：3.0.2 的 `sign`/`verify` 仍用 1.x 解析器，对 2.0 包失败。发布流程
> 在修复前可以先用 `build` 分发未签名包（策略允许时），并把 hash 记录在发布说明。

## 29.4 市场

官方市场是 Git 仓库：

~~~text
bsh_extensions/
  index.json
  packages/<id>/<version>/...
~~~

~~~bash
$BSH ext market
$BSH ext market install com.example.hello
~~~

私有源：

~~~bash
$BSH ext registry add internal https://extensions.example.com
$BSH ext search hello --registry internal
~~~

## 29.5 发布检查清单

- [ ] 版本号已提升（语义化）；
- [ ] `compatibility.host` 与 `extension_api` 反映真实支持范围；
- [ ] 能力列表最小化，README 解释每个高风险能力；
- [ ] `resources` 有合理超时/输出上限；
- [ ] `validate` / `test` / `mock` 全过；
- [ ] 在干净 root 安装并实际使用过；
- [ ] 归档里没有 `target`、`node_modules`、密钥；
- [ ] 记录 hash；
- [ ] 有卸载/回滚说明。

## 29.6 版本与兼容

- 破坏工具 id / 参数 schema = 破坏 API；
- 提升 `extension_api` 前确认宿主范围；
- 依赖要求随版本同步改；
- 保留旧版本目录，`rollback` 才有意义。

## 29.7 练习

1. 为一个包含 wasm 的包写发布说明（构建命令、hash、能力）。
2. 设计私有 registry 的目录结构。
3. 为什么签名不能替代能力审查？

---

<a id="ch30"></a>
# 第 30 章 内建扩展与第一方扩展

**学习目标**：知道宿主自带哪些扩展；理解"扩展优先"的工程规则；判断新功能该放哪。

## 30.1 内建扩展

宿主链接了三个内建扩展（`bsh ext list` 可见）：

| id | 名称 | 工具 |
|---|---|---|
| `com.bos.bsh.annotations` | Workspace Annotations | 无 |
| `com.bos.bsh.diagnostics` | Diagnostics | `diagnostics__graph`、`diagnostics__why` |
| `com.bos.bsh.extensions` | Extensions | `extensions__list` |

它们：

- 运行时是 `builtin`；
- 信任级别 `builtin`，能力默认全放行；
- 不走包存储的 `installed/`，根显示为 `<builtin>`；
- 用户包**不能**声明 `runtime.kind = "builtin"`（`test` 会拒绝）。

## 30.2 扩展优先规则

BSH 的工程规则是：**每个新功能优先做成第一方扩展**，走与第三方相同的清单、
能力、生命周期与效果面。需要新宿主接缝时，优先把它暴露成类型化服务或事件；
加"核心内特权捷径"需要 RFC 与迁移计划。

这条规则的好处：

- 第一方与第三方用同一套测试与审查；
- 能力、审计、生命周期没有"后门"；
- 新能力可以被别的扩展组合。

## 30.3 什么时候必须 builtin

- 需要宿主进程内状态（UI 模型、会话、设置）；
- 需要新的接缝，而现有服务/事件表达不了；
- 性能关键、不能付出 IPC。

即使 builtin，也要：

- 有 id、版本、描述；
- 声明贡献与能力；
- 有测试与文档；
- 能被 disable。

## 30.4 判断练习

1. "把当前工作区的 TODO 汇总成工具"——扩展还是内建？
2. "让面板能播放音频"——扩展还是内建？缺什么接缝？
3. "读取应用内设置"——能否做成服务？谁提供？

---

<a id="ch31"></a>
# 第 31 章 用 ferris-sdk 编写 Rust 扩展

**学习目标**：用 SDK 的 `Extension` trait 写一个 Rust 扩展；用 `ManifestBuilder`
构造清单；用 `TestHarness` 测试；理解它与 BSH 包的关系。

## 31.1 适用场景

ferris-sdk 是**宿主作者**与**第一方扩展**的入口：扩展被编译进宿主，按 id 注册为
`builtin` 运行时。第三方分发的 `extension.toml` 包不能声明 `builtin`，所以：

- 给 BSH 加第一方能力 → SDK + 宿主注册；
- 写可分发扩展 → 第 5–16 章的包格式；
- 自己嵌入 Ferris 做产品 → SDK 的 host facade。

## 31.2 最小扩展

~~~rust
use ferris_sdk::prelude::*;

struct Echo;

#[async_trait]
impl Extension for Echo {
    fn manifest(&self) -> ExtensionManifest {
        ManifestBuilder::new("com.example.echo", "Echo", "1.0.0", "Echo input")
            .capability("agent.tools.register")
            .contribution(ContributionSpec::tool("echo"))
            .build()
            .expect("manifest")
    }

    async fn start(&self, _ctx: &ExtensionContext) -> Result<Vec<Contribution>> {
        Ok(vec![ContributionSpec::tool("echo")
            .into_contribution(json!({ "description": "Echo" }))])
    }

    async fn invoke(&self, call: &Call, _ctx: &InvokeContext) -> Result<Value> {
        Ok(call.input.clone())
    }
}
~~~

要点：

- `manifest()` 返回已校验的 2.0 清单；
- `start()` 返回运行期贡献（工具描述等）；
- `invoke()` 按 `call.kind` / `call.id` 分发；
- `shutdown()` 可选。

## 31.3 ManifestBuilder

覆盖完整 2.0 清单：

| 方法 | 作用 |
|---|---|
| `new(id, name, version, description)` | 身份 |
| `author` / `license` / `repository` / `homepage` | 可选身份 |
| `host` / `extension_api` | 兼容性 |
| `runtime(kind)` / `runtime_config(key, toml)` | 运行时 |
| `capability` | required 能力 |
| `contribution` | 贡献 |
| `dependency` | 依赖 |
| `provide` / `require` | 服务 |
| `max_memory_mb` / `max_execution_ms` / `max_concurrent_calls` / `max_output_bytes` | 资源 |
| `configuration_schema` / `configuration_defaults` | 配置 |
| `startup` | 启动策略 |

`ContributionSpec` 有 16 个构造器（tool、command、panel、prompt、context、
skill、workflow、theme、preset、model_provider、agent_provider、language、
integration、subscribe、publish、hook），每条对应清单里的一个 kind。

## 31.4 运行期贡献

清单声明 `tool("echo")`；`start()` 再给出描述：

~~~rust
ContributionSpec::tool("echo")
    .into_contribution(json!({ "description": "Echo", "input_schema": {...} }))
~~~

`into_contribution` 的 payload 是该 kind 的宿主载荷。

## 31.5 适配与测试

~~~rust
let harness = TestHarness::new(Echo, std::path::PathBuf::from("/tmp/echo"))?;
let contributions = harness.start().await?;
let value = harness.invoke("echo", json!({ "ok": true })).await?;
harness.shutdown().await?;
~~~

`ExtensionRuntime<E>` 把作者接口适配成低层 `Runtime`：`kind()` 从清单推导，
`start/invoke/shutdown` 转发。宿主把它按 id 注册即可。

测试里覆盖：

- 清单字段；
- 贡献数量与 id；
- invoke 的分发与错误；
- shutdown 幂等。

## 31.6 错误

`ferris_sdk::error` 提供 `runtime`、`contribution`、`invalid_input`、
`with_details`，都返回 `ExtError { code, message, details }`。错误码是宿主可扩展
的字符串：不要依赖具体 code 做安全决策。

## 31.7 示例

~~~bash
cargo run -p ferris-sdk --example echo
cargo run -p ferris-sdk --example toolkit
~~~

- `echo.rs`：最小工具 + harness；
- `toolkit.rs`：所有清单字段、贡献 kind、服务、资源、配置与启动策略。

## 31.8 练习

1. 把 `Echo` 改成大小写转换工具；
2. 加一个 `command` 贡献，并让 `invoke` 处理 `kind == "command"`；
3. 用 `TestHarness` 断言未知 kind 的错误。

---

<a id="ch32"></a>
# 第 32 章 平台架构与贡献租约

**学习目标**：理解管理器、贡献 sink、租约与回滚；知道为什么 BSH 只依赖 SDK；
能判断一个改动该落在哪一层。

## 32.1 分层与依赖方向

~~~text
nbos / jsbos  →  agent  →  react  →  bus  →  logging  →  config
                    ↑
                 bsh（宿主适配）→ ferris-sdk → ferris-runtime → ferris-core
~~~

Ferris 三 crate 不依赖 bsh、agent、GPUI：

- `ferris-core`：清单、包、能力、服务、事件、UI、签名、生命周期；
- `ferris-runtime`：标准运行时 + 桥；
- `ferris-sdk`：作者 API + 宿主门面。

CI 用 `scripts/ci/check-ferris-independence.sh` 与 `ferris-smoke` 守住这条线。

## 32.2 管理器拥有什么

`ExtensionManager` 拥有 install / plan / apply / reload：

- `plan` 解析依赖、服务与激活顺序（无副作用）；
- `apply` 构建运行时、启动、收集贡献；
- `reload` 是对新 plan 的 diff，逐个 cancel/revoke；
- 每个 root 一个管理器，内部串行 + 跨进程锁。

## 32.3 贡献 sink 与租约

~~~text
manager.apply
   → 构建 runtime
   → ContributionSink::install(...)
        → 装工具/命令/面板/提示/技能
        → 返回 lease
   → Activation 持有 runtime + lease
   → unload: lease.revoke() → runtime.shutdown()
~~~

BSH 的实现：

- `HostContributionSink` 安装 agent 贡献并返回 `ContributionLease`；
- `BshLeaseHost` 负责从 agent/registry 移除；
- `ActivationList` 在 drop 时释放；
- apply/reload 失败会回滚已启动的 activation，不泄漏运行时或租约。

## 32.4 为什么租约重要

没有租约的注册会泄漏：包被禁用/卸载后，工具还在模型列表里。租约把"注册"变成
**有所有者的、可撤销的**资源，撤销按注册逆序进行。

## 32.5 事件与钩子的位置

- 事件走 `EventBus`（有界、按 topic 过滤）；
- 钩子走 `ferris-runtime::hooks` 的全局队列 + 目标注册表；
- 两者都不在 `react` 的 trait 接缝上：直接请求/响应（LLM、工具）走
  `LlmClient`/`Tool`，agent 间消息走 `bus`。

## 32.6 改动落点

| 你想改 | 落点 |
|---|---|
| 清单字段/校验 | `ferris-core` |
| 一个标准运行时 | `ferris-runtime` |
| 作者 API | `ferris-sdk` |
| agent/UI 接线、CLI | `crates/bsh` |
| 平台原语 | `crates/bus` / `crates/react` / `crates/agent` |

跨层改动要先问：这能不能做成扩展、服务或事件？宿主特权捷径是例外，不是默认。

## 32.7 练习

1. 画一次失败 apply 的回滚路径。
2. 为什么 `plan` 必须无副作用？
3. 如果要在工具卸载后保留它的历史轨迹，应该存在哪一层？

---

<a id="ch33"></a>
# 第 33 章 从 manifest 1.x 迁移

**学习目标**：把 1.x 包迁到 2.0；知道映射关系与迁移后的验证。

## 33.1 硬切

2.0 是唯一接受的 schema。1.x 包在加载时报：

~~~text
unsupported manifest schema "1.0"; run 'bsh ext migrate' to write 2.0
~~~

## 33.2 迁移命令

~~~bash
$BSH ext migrate <path>            # 打印 2.0 清单，不写文件
$BSH ext migrate <path> --write    # 写回 extension.toml，保留 extension.toml.v1
~~~

`--write` 只在备份不存在时创建备份，避免覆盖第一次迁移的原文。

## 33.3 映射表

| 1.x | 2.0 |
|---|---|
| `compatibility.bsh` | `compatibility.host` |
| `runtime.kind` | `runtime.kind` |
| `runtime.entry/args/env/protocol/startup` | `runtime.config.*` |
| `[contributes] tools/commands/...` | `[[contribution]]` 条目 |
| `contributes.events` | `kind = "event.subscribe"` |
| `contributes.publishes` | `kind = "event.publish"` |
| `contributes.hooks` | `kind = "hook"` |
| `capabilities.required` | `capabilities.required`（字符串） |
| `contributes.*` 的 id 列表 | 每条一个贡献，payload 为空表 |

注意：

- 迁移后的 payload 是空表；工具描述、命令正文仍从文件读；
- 事件/钩子的 id 用主题/点名；
- 迁移不会重写代码，只重写清单。

## 33.4 迁移后检查

~~~bash
$BSH ext validate <path>
$BSH ext test <path>
$BSH ext graph <path>
$BSH ext mock <path> <tool> '<json>'
~~~

重点看：

- 贡献数量与 1.x 的列表一致；
- 运行时配置键都进了 `runtime.config`；
- 事件订阅/发布、钩子点没有丢；
- 依赖/服务要求仍是 semver。

## 33.5 旧 fixture

`crates/ferris-core/testdata/legacy` 里仍是 1.x，**故意**保留用于迁移测试。
不要把它们"顺手升级"。

## 33.6 练习

1. 迁移一个带 20 个事件订阅的包，核对数量。
2. 迁移一个 process 包，确认 `entry/args` 进入 `runtime.config`。
3. 迁移后删掉 `extension.toml.v1` 再迁移一次，会发生什么？

---

<a id="ch34"></a>
# 第 34 章 12 周课程大纲

**适用**：本科高年级/研究生课程、企业内训、读书会。按每周 3 小时讲解 + 3 小时
实验设计；总学时约 72。

## 34.1 周计划

| 周 | 主题 | 阅读 | 实验 | 交付 |
|---|---|---|---|---|
| 1 | 平台与第一个包 | 第 1–2 章 | L1 | 能 validate/mock/install 的 hello |
| 2 | 清单与贡献 | 第 3–4 章 | 改坏清单、修好 | 贡献清单表 |
| 3 | 工具与 schema | 第 5 章 | L2 | 三个工具的包 |
| 4 | 软表面 | 第 6 章 | 命令/上下文/技能 | 带命令与上下文的包 |
| 5 | 面板与 UI | 第 7 章 | 静态面板 + 动作 | 可交互面板 |
| 6 | 事件、配置、存储 | 第 8–9 章 | 配置驱动工具 | 配置/存储实验报告 |
| 7 | process 运行时 | 第 10、12 章 | L3 | 进程工具 + 握手排错 |
| 8 | MCP 与运行时对比 | 第 13、15、16 章 | MCP echo | 运行时选型报告 |
| 9 | Extism | 第 14 章 | L4 | wasm 工具 |
| 10 | 能力、生命周期、分发 | 第 17–20 章 | 授权/审计/打包 | 威胁模型 + 归档 |
| 11 | 钩子与观测 | 第 22–25 章 | L5 | 观测器 + 面板 |
| 12 | 测试、发布、答辩 | 第 26–30 章 | L6 | 结课项目与演示 |

## 34.2 每周教学节奏

1. **20 分钟回顾**：上周实验的失败案例（匿名）；
2. **60 分钟讲授**：本章概念 + 现场演示；
3. **20 分钟读代码**：打开对应源码文件，指出文档与代码的对应关系；
4. **90 分钟实验**：两人一组，教师巡场；
5. **10 分钟收尾**：写下"今天最反直觉的一点"。

## 34.3 讲师准备

- 一个干净仓库 + 预构建的 `bsh` 二进制；
- 每组一个 `--root` 目录，避免互相污染；
- 离线环境的 wasm 依赖预下载；
- 一份"已知偏差"（附录 I）发给大家，避免把缺陷当作业错误；
- 演示机不要登录真实模型账号；用假 key 演示拒绝路径。

## 34.4 常见学生错误（提前讲）

| 错误 | 预防 |
|---|---|
| 以为清单声明=授权 | 第 1 周就演示 `why` 拒绝 |
| 在 declarative 里想读文件 | 第 3 周演示边界 |
| 忘记 frontmatter | 第 4 周给模板 |
| 面板 action 写资格名 | 第 5 周强调"动作是贡献 id" |
| 进程 stdout 打日志 | 第 7 周演示握手失败 |
| 把钩子当拦截器 | 第 11 周明确 observe only |

## 34.5 考核权重

- 实验 40%（6 次，按验收点）
- 结课项目 40%（功能/安全/测试/文档）
- 课堂与互评 20%

---

<a id="ch35"></a>
# 第 35 章 实验手册

每个实验都给：目标、步骤、验收、故意失败。

## L1 第一个包

**目标**：走通 validate → test → mock → install → info。

~~~bash
cargo build -p bsh
export BSH=./target/debug/bsh
$BSH ext new com.example.lab1 --dir /tmp/lab1
$BSH ext validate /tmp/lab1
$BSH ext test /tmp/lab1
$BSH ext mock /tmp/lab1 echo '{"text":"hello"}'
$BSH ext install /tmp/lab1 --root /tmp/lab1-root
$BSH ext info com.example.lab1 --root /tmp/lab1-root
~~~

**验收**：`validate` 打印 valid；`mock` 输出参数；`info` 的 tools 行为
`lab1__echo`。

**故意失败**：删掉 `tools/echo.json`，再 validate；记录错误原文。

## L2 三个工具与授权

**目标**：三种 declarative 实现 + 能力拒绝。

1. 加 `version`（constant）、`greet`（template）、`write`（template +
   `workspace.write`）；
2. 声明并在清单加 `workspace.write` 到 required；
3. 安装后看 `info` 的 capability 行；
4. 授权后重新 `reload`，再看。

**验收**：未授权时 info 显示 denied；授权后 allowed；mock 始终能跑。

**故意失败**：把 `workspace.write` 写成 `workspace.write!`，读校验错误。

## L3 进程工具

**目标**：实现协议、排错。

1. 复制 `examples/extensions/process-echo`；
2. 加一个 `sum` 工具（服务端计算）；
3. 手工回放 initialize；
4. 安装并调用。

**验收**：`tools/call` 返回正确；`max_execution_ms` 生效。

**故意失败**：initialize 不回显 token；stdout 混入一行日志；entry 写绝对路径。

## L4 Extism 工具

**目标**：构建并运行 wasm 插件。

1. 用 `extism-echo/guest` 模板；
2. 加 `storage_set/get` 计数；
3. 构建 wasm，替换 `plugin.wasm`；
4. validate/test/mock。

**验收**：`mock` 返回计数；第二次调用计数递增。

**故意失败**：把 `bsh_host` op 写错；不导出 `bsh_call_tool`。

## L5 观测器

**目标**：端到端钩子。

1. 安装 `extism-hook`；
2. 不授权，`why`；
3. 授权四项能力，`why` applied；
4. 跑一轮对话；
5. 打开面板/调用 stats。

**验收**：计数器增长；移除授权后不再增长。

**故意失败**：声明 `model.telepathy`；process 包声明 hook；只授权
`extension.storage`。

## L6 发布

**目标**：打包、审查、回滚。

1. 提升版本到 1.1.0；
2. build 出归档；
3. 在干净 root 安装；
4. 再装 1.0.0，versions/rollback；
5. 写能力审查说明。

**验收**：归档可安装；rollback 生效；审查表完整。

**故意失败**：归档里塞 `target/`；hash 与发布说明不一致。

---

<a id="ch36"></a>
# 第 36 章 习题与参考答案要点

答案要点用于教师批改，不要求逐字。

## 第 1–4 章

1. **声明与授权的关系**：声明是契约与风险面；授权是用户/策略决定；调用时还要
   按作用域再检查。
2. **qualify 计算**：`data-tools__csv_read`。
3. **只有 prompt/skill 的包**：能应用；TOOLS 列为空；agent 构建仍会安装提示/技能。
4. **面板 action 与命令**：先找同名工具；没有则 `runtime.command`；
   declarative 没有 `command` 实现，会报不支持。
5. **command 双重身份**：文件是插入模板（内容型）；运行时 `command()` 是另一个
   可执行接缝（面板回退、集成、CLI）。

## 第 5–9 章

6. **sum 工具**：declarative 的模板不能算术；需要 process/wasm/extism。
7. **默认值**：模板缺失渲染空串，不能表达默认值；换运行时或在 schema 层给默认。
8. **图片限制**：只画 `asset/`；远程图会引入 SSRF/隐私/离线可用性问题。
9. **主题非法色**：解析失败，回退宿主调色板；不导致包失败。
10. **工作流引用外包工具**：加载期报 missing tool（工作流只允许同包）。
11. **配置默认值不能放秘密**：默认值随包分发、可被读取。

## 第 10–16 章

12. **读 Git 状态 + 写文件**：process（依赖 git CLI）或 extism/wasm（需实现）；
    process 最简单，但需要 `process.execute` 与 `workspace.write`。
13. **复用 MCP**：`mcp`。
14. **观测器**：extism（本构建唯一钩子运行时）；process 收不到钩子。
15. **进程握手失败**：入口/权限/协议版本/token/外层沙箱五类。
16. **WASM 能力位置**：在 host 导入函数里，不在工具描述符。
17. **Cordis 需要 process.execute**：桥是子进程。

## 第 17–21 章

18. **agent.tools.register 仍需声明**：注册会改变模型可见面，是低风险但非零。
19. **域名作用域**：同域通过；异域拒绝。
20. **hooks.observe 高风险**：能看到 prompt 形状、工具参数、HTTP 元数据。
21. **quarantine vs uninstall**：隔离保留数据、移走代码、禁用；卸载默认也保留数据。
22. **服务身份用 contract**：name 只是本地别名，contract 才是跨包协议。

## 第 22–25 章

23. **observe 不能 deny**：设计上 intercept 才能；需要有序链、超时、失败策略。
24. **200 chunk**：≤64/批 → 至少 4 次 guest 调用（同一 point）。
25. **没导出 bsh_hook**：每次投递 warn 失败，不影响调用方。
26. **mock 零计数**：不跑 agent，没有 model/tool 接缝。

## 第 26–33 章

27. **mock 不能替代宿主测试**：不查授权、不跑钩子/事件、不渲染面板。
28. **plan 无副作用**：只读解析，允许在安装前预览与比较。
29. **签名边界**：证明发布者与完整性，不证明行为。
30. **builtin 不能由用户包声明**：`test` 明确拒绝。

---

<a id="ch37"></a>
# 第 37 章 结课项目与评分量规

## 37.1 题目

任选一个真实工作流，做成可安装的 BSH 扩展：

- 工作区笔记/知识卡；
- 代码审查辅助；
- 外部 API 查询；
- 观测/成本面板；
- 语言诊断；
- 团队规范检查。

硬性要求：

1. manifest 2.0，能 `validate`/`test`；
2. 至少两个工具，其中一个有 schema 与明确失败语义；
3. 至少一个命令或面板；
4. 至少一条拒绝路径的测试或演示；
5. 能力最小化，README 解释每个高风险能力；
6. 有构建/安装/卸载说明；
7. 可打包成归档。

## 37.2 评分量规（100 分）

| 维度 | 分值 | 优秀 | 合格 |
|---|---:|---|---|
| 功能完整性 | 30 | 真实可用，错误可恢复 | 主路径可用 |
| 安全与权限 | 20 | 最小能力、作用域窄、拒绝路径清晰 | 能跑但权限偏大 |
| 测试 | 15 | 覆盖成功/拒绝/边界，可重复 | 只有手工验证 |
| 用户体验 | 15 | 描述清楚、面板/命令自然 | 能用但需解释 |
| 文档 | 10 | 安装、能力、限制、示例齐全 | 有 README |
| 发布 | 10 | 版本、归档、hash、回滚说明 | 能装 |

## 37.3 答辩（10 分钟）

1. 演示主路径（2 分钟）；
2. 演示一次拒绝/失败（2 分钟）；
3. 解释能力与威胁模型（3 分钟）；
4. 回答"为什么不选另一个运行时"（1 分钟）；
5. 回答"如果用户量 ×100，哪里会坏"（2 分钟）。

## 37.4 评审清单

见附录 G。教师重点看：

- 有没有把高风险能力藏在"方便"里；
- 有没有把 mock 当测试；
- 有没有解释运行时选择；
- 卸载后是否留下无法清理的状态。

---

<a id="appA"></a>
# 附录 A manifest 2.0 速查

## A.1 骨架

~~~toml
schema_version = "2.0"          # 必须

[extension]
id = "com.example.demo"         # 反向域名
name = "Demo"
version = "1.0.0"
description = "What it does"
authors = ["You"]               # 可选
license = "MIT"                 # 可选
repository = "https://..."      # 可选
homepage = "https://..."        # 可选

[compatibility]
host = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"            # builtin/declarative/process/mcp/extism/wasm-component/cordis

[runtime.config]
startup = "on-demand"           # on-startup/on-demand/on-workspace-open/manual
# entry = "server.py"           # process/mcp/extism/wasm/cordis
# args = ["--stdio"]            # process/mcp
# protocol = "jsonrpc-2.0"      # process；mcp 用 "mcp"
# [runtime.config.env]
# KEY = "value"

[capabilities]
required = ["agent.tools.register"]   # 只放 required

[[contribution]]
kind = "tool"
id = "echo"

[contribution.payload]          # 可空

[configuration]
schema = "schemas/config.schema.json" # 可选

[configuration.defaults]
verbose = false

[resources]
max_memory_mb = 256
max_execution_ms = 30000
max_concurrent_calls = 4
max_output_bytes = 1048576

[dependencies]
# "com.example.base" = "^1.0"

[services]
provides = []
requires = []

# [[services.provides]]
# name = "com.example.search"
# contract = "com.example.search-api"
# version = "1.0.0"
# scope = "workspace"
# capabilities = []

# [[services.requires]]
# name = "com.example.config"
# contract = "com.example.config-api"
# version = "^1.0"
# scope = "extension"
# optional = true
~~~

## A.2 贡献 kind 与文件

| kind | 文件 | 必需字段 |
|---|---|---|
| tool | tools/<id>.json | implementation（declarative） |
| command | commands/<id>.md | 正文 |
| panel | panels/<id>.json | title、view |
| prompt | prompts/<id>.md | 正文 |
| context | contexts/<id>.md | 正文 |
| skill | skills/<id>/SKILL.md | SKILL.md |
| workflow | workflows/<id>.json | steps |
| theme | themes/<id>.json | name、colors |
| preset | presets/<id>.json | name |
| model-provider | providers/models/<id>.json | name、base_url |
| agent-provider | providers/agents/<id>.json | name |
| language | languages/<id>.json | name、extensions、tool |
| integration | integrations/<id>.json | name、kind、action |
| event.subscribe | 无 | 主题 |
| event.publish | 无 | 主题 |
| hook | 无 | 点名 |

## A.3 限额速查

| 对象 | 上限 |
|---|---|
| 清单 | 256 KiB |
| 单个工具/贡献文件 | 256 KiB |
| 配置文件 | 256 KiB |
| 包 | 256 MiB / 10000 文件 |
| UI 文档 | 1 MiB；节点 1000；深度 32；单文本 16 KiB；总文本 256 KiB |
| 事件 | 256 KiB；每 source 200/s；订阅队列默认 64 |
| 进程/MCP 单行 | 4 MiB |
| Extism/WASM 桥 body | 8 MiB |
| 钩子队列/批次/字段 | 1024 / 64 / 约 8K 字符 |
| 工具描述 | 4096 字节 |
| 面板标题 | 120 字符 |
| 扩展 id | 128 字节；每段 63 |
| 资源默认 | 256 MiB / 30000 ms / 4 并发 / 1 MiB 输出 |

---

<a id="appB"></a>
# 附录 B 能力矩阵

| 能力 | 风险 | builtin | verified | community/local | 备注 |
|---|---|---|---|---|---|
| workspace.read | low | 允许 | 允许 | 允许 | 路径作用域 |
| workspace.write | high | 允许 | 需显式 | 需显式 | 路径包含检查 |
| network.http | high | 允许 | 需显式 | 需显式 | 域名/方法 |
| process.execute | critical | 允许 | 需显式 | 需显式 | 程序/参数 |
| computer.use | critical | 允许 | 需显式 | 需显式 | 屏幕/输入 |
| secrets.read | critical | 允许 | 需显式 | 需显式 | 秘密名 |
| extension.storage | low | 允许 | 允许 | 允许 | 私有命名空间 |
| agent.tools.register | low | 允许 | 允许 | 允许 | 注册非授权 |
| agent.context.provide | low | 允许 | 允许 | 允许 | 预算内 |
| agent.prompts.provide | low | 允许 | 允许 | 允许 | 预算内 |
| agent.skills.provide | medium | 允许 | 允许 | 允许 | 标准特例 |
| agent.models.provide | medium | 允许 | 允许 | 需显式 | |
| agent.agents.provide | medium | 允许 | 允许 | 需显式 | |
| agent.workflows.provide | medium | 允许 | 允许 | 需显式 | |
| events.publish | low | 允许 | 允许 | 允许 | 主题声明 |
| events.subscribe | low | 允许 | 允许 | 允许 | 主题声明 |
| ui.panels | low | 允许 | 允许 | 允许 | |
| ui.commands | low | 允许 | 允许 | 允许 | |
| ui.theme | low | 允许 | 允许 | 允许 | |
| hooks.observe | high | 允许 | 需显式 | 需显式 | 仅 Extism |
| agent.intercept | high | 允许 | 需显式 | 需显式 | 事件拦截；钩子 intercept 未实现 |

> "允许"指默认策略；用户仍可 revoke。显式授权用
> `bsh ext grant <id> <cap> [--scope ...]`。

---

<a id="appC"></a>
# 附录 C 钩子载荷参考

## C.1 外层

~~~json
{"point": "model.request", "batch": [ ... ]}
~~~

`batch` 只含同一个 point 的事件，最多 64 条。

## C.2 model.request

~~~json
{"agent":"assistant","data":{
  "model":"gpt-4o",
  "request_id":"req-...",
  "request":"{\"request_id\":\"req-...\",\"model\":\"gpt-4o\",\"api_mode\":\"chat\",\"temperature\":0.7,\"max_tokens\":null,\"prompt_bytes\":1834,\"messages\":[{\"role\":\"system\",\"bytes\":1200,\"preview\":\"You are...\"}]}"
}}
~~~

`messages` 最多 48 条；`preview` ≤512 字符。

## C.3 model.response

完整：

~~~json
{"agent":"assistant","data":{
  "request_id":"req-...",
  "response_type":"react",
  "response":"{\"response_type\":\"react\",\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":40,\"total_tokens\":160}}"
}}
~~~

流式步骤：

~~~json
{"agent":"assistant","data":{
  "request_id":"req-...",
  "response_type":"stream",
  "response_bytes":"320",
  "response_text":"...",
  "had_tool_call":"false"
}}
~~~

## C.4 tool.before / tool.after

~~~json
{"agent":"assistant","data":{
  "tool_name":"hello__echo","call_id":"call-1","tool_args":"{\"text\":\"hi\"}"
}}
~~~

~~~json
{"agent":"assistant","data":{
  "tool_name":"hello__echo","call_id":"call-1","tool_result":"{\"text\":\"hi\"}"
}}
~~~

## C.5 http.exchange

~~~json
{
  "source":"com.example.extism-echo",
  "request_id":"req-...",
  "method":"POST",
  "host":"api.example.com",
  "path":"/v1/chat",
  "status":200,
  "request_bytes":120,
  "response_bytes":3400,
  "content_type":"application/json",
  "ttfb_ms":41,
  "total_ms":90,
  "error":null
}
~~~

没有 headers、body、SSE 帧统计；`path` 不含 query。

## C.6 限额

| 项 | 值 |
|---|---|
| 队列 | 1024 |
| 批次 | ≤64 |
| 单字段 | 约 8K 字符 |
| 点 | 5 个 |
| 每包声明 | ≤32 |
| 脱敏 | `sk-` / `ghp_` / `Bearer ` |

---

<a id="appD"></a>
# 附录 D 错误目录

| 变体 | 典型消息 | 含义 | 处理 |
|---|---|---|---|
| Io | `<path>: <os error>` | 文件不存在/不可读 | 补文件、查路径 |
| Toml | `<path>: invalid TOML: ...` | 清单 TOML 语法/字段 | 看行列与"allowed fields" |
| Json | `<path>: invalid JSON: ...` | 贡献 JSON 语法 | 用 `jq` 校验 |
| Manifest | `<path>: <message>` | 校验失败 | 读 message 字段名 |
| InvalidId | `invalid extension id ...` | id 语法 | 反向域名、小写 |
| InvalidContribution | `invalid contribution id ...` | 贡献 id 语法 | 点分、小写 |
| InvalidVersion | `invalid version ...` | semver 错 | 修版本 |
| Incompatible | 兼容性错误 | host/api 范围不满足 | 改兼容块或升级宿主 |
| Lifecycle | `cannot move from X to Y` | 状态机拒绝 | 按当前状态操作 |
| UnsupportedRuntime | `runtime X is not implemented` | 本构建没有该运行时 | 换运行时或启用 feature |
| MissingCapability | 缺能力 | 声明/授权缺失 | 加声明或 grant |
| Denied | `not authorized for ...` | 策略拒绝 | 看作用域与理由 |
| Dependency | `dependency error ...` | 依赖不满足 | 先装依赖 |
| Cycle | `dependency cycle: ...` | 循环 | 打破环 |
| Runtime | `runtime error: ...` | 运行时内部错 | 看日志 |
| Remote | `reported: ...` | 远端返回错误 | 看 server 响应 |
| Protocol | `protocol error: ...` | 握手/行协议错 | 对照协议章节 |
| Timeout | `timed out after N ms` | 调用超时 | 调限额/优化 |
| Limit | `exceeded a limit: ...` | 超上限 | 裁剪/调上限 |
| NotInstalled | `is not installed` | 未安装 | install |
| NotFound | `X was not found` | 工具/贡献不存在 | 对齐 id |
| AlreadyInstalled | `version already installed` | 重复安装 | 升版本或卸载 |
| NoMatchingVersion | `no package provides ...` | 版本约束无解 | 放宽要求/换版本 |
| Corrupt | `<path>: <message>` | 文件损坏/哈希不符 | 重装/doctor |
| NameConflict | `name ... already claimed` | 资格名冲突 | 改名或卸载冲突包 |

错误码字符串是宿主可扩展的；不要用错误文本做程序判断，用结构与状态。

---

<a id="appE"></a>
# 附录 E 面板节点参考

UI 文档：

~~~json
{"schema_version": "1.0", "root": <node>}
~~~

每个节点的 `type` 是 kebab-case，字段如下。

## E.1 布局

~~~json
{"type":"column","children":[]}
{"type":"row","children":[]}
{"type":"container","children":[]}
{"type":"divider"}
{"type":"spacer","size":8}
~~~

## E.2 文本

~~~json
{"type":"text","text":"..."}
{"type":"heading","text":"...","level":2}
{"type":"markdown","text":"..."}
{"type":"code","language":"rust","value":"..."}
~~~

## E.3 交互

~~~json
{"type":"button","label":"Run","action":"run","style":"primary"}
{"type":"input","label":"Name","value":"","placeholder":"...","action":"submit"}
{"type":"select","label":"Mode","value":"a","options":[{"value":"a","label":"A"}],"action":"change"}
{"type":"checkbox","label":"Enable","checked":true,"action":"toggle"}
~~~

`style`：`primary` / `secondary`（默认）/ `danger` / `ghost`。

## E.4 数据

~~~json
{"type":"list","items":[{"title":"Row","detail":"...","action":"open","checked":false}]}
{"type":"table","columns":["A","B"],"rows":[["1","2"]]}
{"type":"tree","nodes":[{"id":"n1","label":"Root","children":[]}]}
{"type":"tabs","tabs":[{"id":"t1","label":"One","content":{"type":"text","text":"..."}}]}
{"type":"progress","value":0.4,"label":"40%"}
~~~

## E.5 媒体与提示

~~~json
{"type":"image","src":"asset/logo.png","alt":"Logo"}
{"type":"notification","level":"warning","text":"..."}
~~~

- `image.src` 只接受 `asset/...`；
- `notification.level`：`info` / `warning` / `error` / `success`。

## E.6 动作语义

- `action` 是贡献 id，不是资格名；
- 先找同名工具，找不到再用运行时 `command`；
- 工具存在时按其 `capabilities` 做每次调用检查；
- 动作带宿主修订号，过期动作会被拒绝。

---

<a id="appF"></a>
# 附录 F 示例索引

| 示例 | 运行时 | 演示什么 |
|---|---|---|
| `hello` | declarative | 最小包：工具、命令、面板、提示 |
| `allinone` | process | 几乎所有贡献 kind、配置、工作流、语言、集成、事件 |
| `bsh-notes` | process | 第一方工作区笔记（保存/列表） |
| `language-python` | process | 语言服务 + GitHub 集成 |
| `process-echo` | process | 最小 JSON-RPC 协议 |
| `mcp-echo` | mcp | MCP 工具/资源/提示映射 |
| `extism-echo` | extism | 工具、命令、面板、提示、上下文、provider、theme、preset、事件 |
| `extism-hook` | extism | observe 钩子 + 存储计数 + 面板 |
| `wasm-echo` | wasm-component | WIT component 工具 |
| `cordis-echo` | cordis | `apply(ctx, config)` 注册工具/命令/技能 |
| `hook-demo` | process | 全部保留主题订阅 + 多种贡献（进程收事件，不收钩子） |
| `computer-use` | process | 屏幕/输入能力的进程示例 |
| `builtin-template` | — | 仅 README，不是可安装包 |

运行前先 `bsh ext validate examples/extensions/<name>`；进程类示例需要
`process.execute`；ferris 进程不套操作系统沙箱。

---

<a id="appG"></a>
# 附录 G 审查清单

## G.1 作者

- [ ] id 合法且稳定，末段唯一；
- [ ] 版本语义化；
- [ ] 每个声明都有文件；
- [ ] 工具描述是动词开头、参数最少；
- [ ] 高风险能力只出现在必要工具；
- [ ] 描述符/清单的能力与代码实际使用一致；
- [ ] 错误返回 `isError` 且可操作；
- [ ] 面板动作指向存在的工具/命令；
- [ ] 钩子只声明需要的点；
- [ ] README 有安装、能力、限制、卸载。

## G.2 安全评审

- [ ] 威胁模型：数据、网络、进程、秘密各一条；
- [ ] 作用域是否最窄；
- [ ] 是否有路径穿越/命令注入/SSRF；
- [ ] 是否把用户数据写进日志/面板；
- [ ] 钩子采集是否包含 prompt/工具参数，是否脱敏；
- [ ] 签名与来源是否可信（若分发）；
- [ ] 卸载/隔离后数据如何处理。

## G.3 发布

- [ ] `validate/test/mock` 全过；
- [ ] 干净 root 安装并真实使用；
- [ ] 归档不含构建产物与密钥；
- [ ] hash 记录；
- [ ] 版本/兼容范围/依赖一致；
- [ ] 回滚说明；
- [ ] 市场/registry 条目更新。

## G.4 教学

- [ ] 实验有验收点；
- [ ] 故意失败覆盖最常见错误；
- [ ] 明确"mock 不证明什么"；
- [ ] 发附录 I，避免把缺陷当作业错。

---

<a id="appH"></a>
# 附录 H 术语表

| 术语 | 含义 |
|---|---|
| 扩展（extension） | 一个带 extension.toml 的包，向宿主贡献能力 |
| 清单（manifest） | extension.toml，schema 2.0 |
| 贡献（contribution） | 一条声明：kind、id、payload |
| 表面（surface） | 贡献在宿主里的呈现：工具、命令、面板、提示… |
| 资格名（qualified name） | 模型可见名，namespace 加双下划线加本地名 |
| 运行时（runtime） | 执行贡献的实现：declarative/process/… |
| 能力（capability） | 受控资源的许可类别（21 项） |
| 作用域（scope） | 能力的具体范围：路径、域名、程序、秘密 |
| 授权（grant） | 把能力授予某扩展的记录 |
| 审计（audit） | 每次授权决策的 JSONL 记录 |
| 生命周期（lifecycle） | discovered 到 active/failed 的状态机 |
| 应用（apply） | 管理器构建运行时并安装贡献 |
| 租约（lease） | 可撤销的贡献注册句柄 |
| 存储（storage） | 扩展私有数据命名空间 |
| 事件（event） | 事后广播，主题加信封 |
| 钩子（hook） | 接缝上的 observe 投递，按点声明、批处理 |
| observe | 只读钩子类，不阻塞、不能否决 |
| intercept | 可 deny/replace 的钩子类（设计，未实现） |
| 契约（contract） | 服务身份，跨包稳定的字符串 |
| 沙箱（sandbox） | 进程/wasm 的 OS 或运行时隔离 |
| fuel | wasm 执行配额 |
| epoch | wasm 墙钟中断机制 |
| 桥（bridge） | Extism/Node 与宿主之间的 JSON 调用层 |
| WIT | WebAssembly 接口类型契约 |
| 市场（market） | Git 仓库形式的扩展索引 |
| 信任级别 | builtin/verified/community/local/unknown |
| 包哈希 | 规范化文件内容的摘要 |
| 签名 | Ed25519 覆盖 id/version/hash |

---

<a id="appI"></a>
# 附录 I 3.0.2 现状核对与已知偏差

本附录记录写作时（2026-11-03，HEAD 5cdad177）用 target/debug/bsh 与源码核对的
结果。**先读这一页**，能避免把实现缺陷误当成自己的错误。

## I.1 已核对的行为

| 行为 | 证据 |
|---|---|
| 脚手架生成 4 个文件、2 个贡献 | bsh ext new 原样输出 |
| validate 打印 hash/schema/contributions | 命令输出 |
| test 打印 tools/commands/panels/workflows 计数 | 命令输出 |
| mock 运行 declarative 三种实现 | 命令输出（含缺失参数） |
| install 输出 source/hash/path | 命令输出 |
| list/info/plan/graph/why/effects/doctor/logs/metrics | 命令输出 |
| grant/revoke/disable/enable/versions/uninstall | 命令输出 |
| 能力词表 21 项 | bsh ext capabilities |
| 未授权 hooks.observe 导致 not applied | why 原文 |
| build 产出 tar.gz + hash | 命令输出 |
| 钩子仅 5 点、仅 Extism | ferris-runtime/src/hooks.rs、manifest.rs |

## I.2 已知缺陷（3 个，均与 2.0 清单解析有关）

### 缺陷 1：bsh ext sign 对 2.0 包失败

~~~bash
$BSH ext sign /tmp/hello-lab --publisher acme --key /secure/acme.key
~~~

~~~text
error: /tmp/hello-lab/extension.toml: invalid TOML: TOML parse error at line 10, column 1
   |
10 | host = ">=3.0.0, <4.0.0"
   | ^^^^
unknown field host, expected one of bsh, extension_api, ui_api
~~~

原因：crates/ferris-core/src/signing.rs:80 的 sign_package 调用 Manifest::load
（1.x 解析器）。

### 缺陷 2：bsh ext verify 同样失败

原因：crates/bsh/src/extension/cli.rs:1096 用 Manifest::load 读包。

### 缺陷 3：bsh ext config 同样失败

~~~bash
$BSH ext config com.example.hello --root /tmp/root
~~~

~~~text
error: .../installed/com.example.hello/0.1.0/extension.toml: invalid TOML: ...
unknown field host, expected one of bsh, extension_api, ui_api
~~~

原因：crates/bsh/src/extension/cli.rs:1602 用 Manifest::load 读已安装包。

### 建议修复（未在本书中改动源码）

把这三处改为 2.0 加载路径，例如：

~~~rust
let manifest = ferris_sdk::manifest_v2::load_package_manifest(&root)?;
~~~

load_package_manifest 会解析 2.0 并转换为宿主内部形状，保留 configuration、
runtime.config、贡献列表。修复后应补三条 CLI 回归测试（2.0 包的 sign/verify/
config）。

---

## I.3 文档与代码的偏差

| 文档 | 偏差 | 以什么为准 |
|---|---|---|
| bsh-hook-system.md | 使用 1.x 的 contributes 表与 hook_options；描述 intercept、hooks.intercept/bodies、process/WASM 钩子 | 代码：5 个 observe 点、仅 Extism、无 intercept 字段 |
| bsh-observability-capture.md | 描述 capture_list/capture_get、frames/max_gap_ms、bodies | 代码：只有 http.exchange 元数据，无帧统计/正文 |
| examples/extensions/extism-hook/README.md | 使用 1.x 的 contributes hooks 列表 | extension.toml 的 2.0 写法 |
| bsh-extension.md | 部分示例仍写 1.x 的 contributes 概念 | 2.0 的 [[contribution]] |

这些是"设计文档/旧示例"的滞后，不是运行时同时支持两套清单：**2.0 是唯一
接受的 schema**。

## I.4 环境相关现象（不是缺陷）

### 进程 mock 在嵌套沙箱中失败

~~~text
error: extension com.example.process-echo protocol error:
process closed stdout before answering
~~~

在已受文件沙箱限制的终端里，macOS 的 sandbox-exec 会拒绝嵌套应用：

~~~text
sandbox-exec: sandbox_apply: Operation not permitted
~~~

手工验证协议仍然可用：

~~~bash
cd examples/extensions/process-echo
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"token":"abc"}}' \
  | python3 server.py
~~~

预期输出：

~~~text
{"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": "1.0", "token": "abc"}}
~~~

正常桌面 BSH 进程不受此限制。

### 进程包显示 failed

同一原因：激活时无法启动子进程。修复环境后 retry 即可。

## I.5 复核方法

~~~bash
git log -1 --format=%H        # 或 jj log -r @-
cargo build -p bsh
./target/debug/bsh ext capabilities
./target/debug/bsh ext new com.example.check --dir /tmp/check
./target/debug/bsh ext validate /tmp/check
./target/debug/bsh ext test /tmp/check
./target/debug/bsh ext mock /tmp/check echo '{"x":1}'
~~~

若这些行为与本书不一致，先看本附录，再查 docs/bsh-extension.md 与
crates/ferris-* 源码；以代码为准。
