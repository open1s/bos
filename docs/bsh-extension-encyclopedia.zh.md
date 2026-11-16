# BSH 扩展大全（4.1.8 / manifest 2.0）

> 这是一本**查阅型**参考手册，不是教程。它把 BSH 扩展平台的每一个契约面——包、manifest、贡献、运行时、能力、事件、钩子、UI、限额、CLI、签名、联邦——压成可检索的表格与规则。
>
> 叙事与实验请配合阅读：
> - [BOS-BSH-Textbook.md](BOS-BSH-Textbook.md)（英文，BSH by Example，本手册的主要依据）
> - [bsh-extension-guide.zh.md](bsh-extension-guide.zh.md)（中文课程式教材，基线较旧，机制仍适用）
> - [bsh-extension.md](bsh-extension.md)（英文规范；与源码冲突时以源码为准）
> - [Ferris Dynamic UI RFC](ferris-dynamic-ui-rfc.md)（surface 1.1 与组合的目标设计）
>
> 基线：BSH 4.1.8，manifest 2.0，extension API 1.x。源码权威位置：`crates/ferris-core`、`crates/ferris-runtime`、`crates/ferris-sdk`、`crates/bsh/src/extension`。
>
> 记号：`必填` / `可选`；限额单位均为字节或毫秒，另有标注除外。`1.1` 指动态 UI 的 surface 文档模式（4.x 新增）。

## 0. 三十秒索引

| 我要… | 看 |
|---|---|
| 写第一个包 | §3 manifest、§4 贡献、§5 工具、附录 A |
| 选运行时 | §11 运行时全表 |
| 解释"为什么没跑起来" | §12 生命周期、§17 排障、附录 G |
| 声明权限 | §13 能力与授权 |
| 做面板 / 动态界面 | §8 面板与动态 UI、附录 C/D |
| 接事件 / 钩子 | §9 事件、§10 钩子、附录 F |
| 签名 / 发布 / 回滚 | §18 打包与签名 |
| 跨机共享能力 | §19 联邦与扩展 |
| 一条命令查全部 | §16 CLI 全命令 |

最小可运行包（declarative）：

```text
hello/
  extension.toml
  tools/echo.json
```

```toml
schema_version = "2.0"

[extension]
id = "com.example.hello"
name = "Hello"
version = "1.0.0"
description = "A minimal declarative extension"

[compatibility]
host = ">=4.0.0, <5.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[capabilities]
required = ["agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"
```

```json
{ "description": "Echo the arguments", "implementation": { "kind": "echo_arguments" } }
```

验证与试跑：`bsh ext validate hello`、`bsh ext test hello`、`bsh ext mock hello echo '{"text":"hi"}'`。

## 1. 心智模型与所有权

### 1.1 五个层

```text
ferris-core       manifest、包仓库、manager、能力策略、服务、事件、UI 契约、终端、签名、生命周期
ferris-runtime    builtin/declarative/process/MCP/Extism/WASM/Cordis 运行时；surface provider 适配器
ferris-sdk        作者 API + 宿主门面（BSH 唯一 Ferris 入口）
crates/bsh        宿主适配：HostSpec、RuntimeResolver、UI、agent 装配
crates/agent      Agent / 工具调用 / 会话
```

依赖方向：`bsh → agent → react → bus → logging → config`；Ferris 三件套不依赖 BSH、agent、GPUI。BSH 只有一个 Ferris 依赖：

```toml
ferris-sdk = { path = "../ferris-sdk", features = ["host-runtime"] }
```

### 1.2 三个平面

| 平面 | 所有者 | 内容 |
|---|---|---|
| 声明 | 包作者 | manifest、贡献文件、工具描述、UI 文档 |
| 执行 | Ferris 运行时 | 进程、WASM、Extism、MCP、Cordis、builtin |
| 呈现 | BSH | 窗口、面板、焦点、主题、快捷键、审计行 |

**包不拥有 UI。** 包给文档；宿主给控件。**运行时不是权限。** 声明能力只是"我需要"；授权发生在每次调用。

### 1.3 一次工具调用

1. 模型请求携带工具名（命名空间形式）与参数。
2. agent 解析到贡献的 handler；UI 触发的动作走同一 seam。
3. 宿主按描述符的 `capabilities` 逐项向策略请求；作用域从参数提取（`path`/`url`/`secret`）。
4. 每次决策写一条审计；拒绝发生在运行时之前。
5. 运行时执行；结果按 `max_output_bytes` 截断；失败映射为工具错误。
6. 完成后发布 `tool.completed` / `tool.failed`；observhook 另行投递。

### 1.4 最容易记错的三件事

- **声明 ≠ 授权**：`[capabilities] required` 只进入审查与授权请求，不产生权限。
- **命令 ≠ 工具**：命令由读者触发；工具由模型触发。一个效果要两个前门，就声明两个贡献、共享 handler。
- **事件 ≠ 钩子**：事件是事后事实、可有损；钩子是同步 seam 的观测载荷，只观测、不能改。

## 2. 包结构、哈希与仓库

### 2.1 目录

```text
my-extension/
  extension.toml             必填，schema 2.0
  package.sig                可选 Ed25519 签名
  tools/<id>.json            工具描述符
  commands/<id>.md           命令模板
  panels/<id>.json           声明式面板（view 可为 1.0 或 1.1）
  prompts/<id>.md            提示片段
  contexts/<id>.md           上下文片段
  skills/<id>/SKILL.md       技能
  workflows/<id>.json        工作流
  themes/<id>.json           主题 token
  presets/<id>.json          预设
  providers/models/<id>.json 模型提供者
  providers/agents/<id>.json Agent 角色
  languages/<id>.json        语言服务
  integrations/<id>.json     外部系统集成
  asset/...                  图标与图片（只能经 asset/ 引用）
```

只读取被贡献声明的文件；路径必须相对包根且不得逃逸；所有文件有大小上限（§15）。

### 2.2 哈希与不可变

包哈希覆盖：**相对路径、字节长度、内容摘要**，排序；排除 `.git`、`.jj`、`target`、`node_modules`；`package.sig` 不参与它签名的哈希。安装后代码不可变，升级是新的版本目录；`extension-data` 默认在卸载后保留，`--purge-data` 才删除。

### 2.3 仓库布局

```text
<root>/
  installed/<id>/<version>/     不可变代码
  extension-data/<id>/          私有存储
  state.toml                    启用/失败/尝试
  registry.json                 已安装版本、哈希、来源
  lock.toml                     依赖锁定
  grants.toml                  能力授权
  audit-<pid>.jsonl             审计
```

安装要经过：校验 manifest → 解析依赖/服务 → 暂存与哈希 → 验签 → 入仓 → 激活。手工改 `installed/` 会让哈希与 registry 不一致，`doctor` 会报篡改。

## 3. manifest 2.0 逐字段

### 3.1 完整骨架

```toml
schema_version = "2.0"

[extension]
id = "com.example.hello"
name = "Hello"
version = "1.0.0"
description = "A minimal declarative extension"
authors = ["Example"]
license = "MIT"
repository = "https://example.com/hello"
homepage = "https://example.com"

[compatibility]
host = ">=4.0.0, <5.0.0"
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

[configuration]
schema = "schemas/config.schema.json"

[configuration.defaults]
verbose = false

[resources]
max_memory_mb = 256
max_execution_ms = 30000
max_concurrent_calls = 4
max_output_bytes = 1048576

[dependencies]
"com.example.base" = "^1.0"

[services]
provides = []
requires = []
```

### 3.2 顶层字段

| 字段 | 必填 | 规则 |
|---|---:|---|
| `schema_version` | 是 | 恰好 `2.0`；1.x 报 unsupported manifest schema，用 `bsh ext migrate` |
| `extension` | 是 | 身份块 |
| `compatibility` | 是 | 宿主与 API 范围 |
| `runtime` | 是 | 运行时种类与不透明 config |
| `capabilities` | 否 | 仅 required |
| `contribution` | 否 | 通用贡献数组，每项 `{kind,id,payload}` |
| `configuration` | 否 | JSON Schema 路径 + TOML 默认值 |
| `resources` | 否 | 内存/执行/并发/输出上限 |
| `dependencies` | 否 | 扩展 id → semver 需求 |
| `services` | 否 | 提供/消费的服务契约 |

未知顶层字段被拒绝；未知贡献 kind 在投影到宿主类型视图时被拒绝。manifest ≤ 256 KiB。

### 3.3 身份

| 字段 | 必填 | 规则 |
|---|---:|---|
| `id` | 是 | 反向域名；全局稳定；用于目录、存储、授权、审计、限定名 |
| `name` | 是 | 人类可读 |
| `version` | 是 | semver |
| `description` | 是 | 一段描述 |
| `authors` / `license` / `repository` / `homepage` | 否 | 元数据 |

### 3.4 运行时与启动

| kind | 载荷 | 标准适配器用的 config 键 |
|---|---|---|
| `builtin` | 按 id 注册的 Rust 运行时 | 无 |
| `declarative` | 包内文件 | `startup` |
| `process` | JSON-RPC 子进程 | `entry, args, env, protocol, startup` |
| `mcp` | stdio 上的 MCP server | `entry, args, env, startup` |
| `extism` | Extism PDK 插件 | `entry, startup` |
| `wasm-component` | WASM Component | `entry, startup` |
| `cordis` | 以 `apply(ctx, config)` 运行的 JS 包 | `entry, startup` |

`startup` ∈ `on-startup | on-demand | on-workspace-open | manual`，默认 `on-demand`。`runtime.config` 对 manifest 不透明，由适配器校验。

WASM 组件额外获得 WASI preview 2 的 clock/random，`std::time` 与 `getrandom` 可用；stdout/stderr 继承；文件系统、环境变量、socket、stdin 不提供；工作区与网络仍走 `bsh:extension/host` 及其能力检查。

### 3.5 能力

能力声明**只有 required**。完整词表与风险见 §13。`terminal.execute` 是 `process.execute` 的历史别名。以 `bsh ext capabilities` 的实时列表为准。

### 3.6 资源

| 字段 | 默认 | 含义 |
|---|---:|---|
| `max_memory_mb` | 256 | 运行时内存上限 |
| `max_execution_ms` | 30000 | 单次执行上限 |
| `max_concurrent_calls` | 宿主默认 | 声明式并发额度（不是池化 promise） |
| `max_output_bytes` | 宿主默认 | 单次结果上限 |

### 3.7 依赖、服务、配置

```toml
[dependencies]
"com.example.base" = "^1.0"

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
```

服务 scope ∈ `host | workspace | session | extension`。发现服务不是授权：消费者仍要持有提供者声明的能力；required 且不可满足会阻塞激活（`optional = true` 除外）。依赖环与不满足需求在激活前拒绝。

配置：`configuration.schema` 为包内 JSON Schema，`configuration.defaults` 为 TOML 默认值；读取/写入用 `bsh ext config <id> [set <json|file>|schema]`。配置错误是加载错误，不静默取默认值。

### 3.8 校验规则速查

- `schema_version` 必须恰好 2.0；
- `id` 反向域名、全局唯一；贡献 id 点分且包内唯一；
- 版本、`compatibility.host`、`extension_api`、依赖需求都是 semver；
- 运行时 kind 必须是已注册字符串；entry 必须包内相对路径；
- 只读取声明文件；未知顶层字段拒绝；manifest 与贡献文件有上限；
- 声明的 hooks 点必须带 `hooks.observe`，且只有 extism/wasm-component 可声明；
- `panel` 的 `theme`/`theme_dark` 必须由同一包贡献。

## 4. 贡献类型全表

每一项在 manifest 里是：

```toml
[[contribution]]
kind = "tool"
id = "echo"

[contribution.payload]
description = "Echo the input"
```

`payload` 是按 kind 校验的不透明 TOML；标准 kind 同时有文件约定。

| kind | 文件 / 含义 | 进入哪一面 |
|---|---|---|
| tool | `tools/<id>.json` | 模型可调用（延迟暴露，见下） |
| command | `commands/<id>.md` | 读者（composer 斜杠命令） |
| panel | `panels/<id>.json` | 读者（右侧/左侧/底部/主区面板） |
| prompt | `prompts/<id>.md` | 模型可读（系统提示） |
| context | `contexts/<id>.md` | 模型可读（按 scope 注入） |
| skill | `skills/<id>/SKILL.md` | 模型可读（目录 + 按需加载） |
| workflow | `workflows/<id>.json` | 读者（录制式多步） |
| theme | `themes/<id>.json` | 读者（token 覆盖） |
| preset | `presets/<id>.json` | 设置层（provider/model/agent 选择） |
| model-provider | `providers/models/<id>.json` | 设置层（模型条目） |
| agent-provider | `providers/agents/<id>.json` | 设置层（Agent 角色） |
| language | `languages/<id>.json` | 宿主（语言归属与诊断） |
| integration | `integrations/<id>.json` | 宿主（外部系统入口） |
| event.subscribe | 订阅主题 | 事件 |
| event.publish | 发布主题 | 事件 |
| hook | 观测点 | 钩子（仅 extism / wasm） |

### 4.1 三条表面规则

| 表面 | kind | 落点 |
|---|---|---|
| 模型可调用 | tool | 构建期注册为命名空间工具；**延迟**进入请求数组，由 `find_tool` 选出后才出现 |
| 模型可读 | prompt / context / skill | 系统提示或技能目录 |
| 读者可见 | command / panel / theme / workflow | 宿主插件注册表与清单 |

工具延迟是刻意设计：一次安装贡献一族工具，而每个 schema 每一步都重发。构建只广告 `find_tool`；被选中的定义下一步才加入请求。要模型一开始就看到某个工具，就在 prompt 里点名它。

命令永不提升为工具；模型操作 reader shell 是另一种授权。若同一效果要两种入口，声明 `tool` 与 `command` 两个贡献、共享 handler。

### 4.2 限定名

工具在模型请求中的名字是 `namespace__contribution`（资格名）；注册表与策略按包 id 限定。命令面板画出的 token 也是这个拼写，因此草稿与解析器不会对不上。手打的限定名解析不到时按名字报错（`slash.commandUnavailable`），该行按普通文本发送，不会静默变成"什么都没发生"。

### 4.3 命令的注入协议

读者选命令时，composer 只写入 `/name ` 注解，不写模板正文。发送时组装一条消息：

```text
<extension_command name="qualified__name">
<模板正文>
</extension_command>
---
<读者继续输入的话>
```

注解在块附上后离开请求；transcript 保留读者写的行；后续每次重放都把同一行还原成同一块（`Runner::seed_session`），而不是裸 `/name`。子 run 的行由它自己的 agent 展平，也能还原成 `/name words`。

### 4.4 技能的两条来源

- 声明文件 `skills/<id>/SKILL.md`：构建期加载；
- 运行时注册：Cordis 包在 `apply(ctx)` 里调用 `ctx.skills.register(..)`，运行时把每个注册物化到 data 目录并把目录广告给宿主。

运行时集合优先；目录为空时回退到声明文件，因此声明过的包不会因运行时集合为空而变差。

### 4.5 贡献的总规则

- id 包内唯一、点分语法；
- 声明了就必须有文件（缺文件是加载错误，不是"少贡献一点"）；
- 路径包内相对、不得逃逸；
- 贡献文件 ≤ 256 KiB；
- 未在 manifest 声明的文件不读取。

## 5. 工具

### 5.1 描述符

`tools/<id>.json`：

| 字段 | 必填 | 含义 |
|---|---:|---|
| `description` | 是 | 给模型看的说明 |
| `input_schema` | 否 | 参数 JSON Schema |
| `capabilities` | 否 | 每次调用要检查的能力（最小权限 seam） |
| `implementation` | declarative 必填 | 纯变换体；其他运行时忽略 |

### 5.2 declarative 的三种实现

**echo_arguments**：原样返回参数。

```json
{ "implementation": { "kind": "echo_arguments" } }
```

**constant**：返回固定值。

```json
{ "implementation": { "kind": "constant", "value": { "ok": true } } }
```

**template**：渲染顶层占位符。

```json
{
  "implementation": {
    "kind": "template",
    "text": "path={{path}} depth={{depth}}"
  }
}
```

字符串参数渲染为文本；其他 JSON 渲染为紧凑 JSON；缺失渲染为空串。**没有条件、循环、嵌套路径**——模板不是编程语言。需要逻辑就换运行时。

### 5.3 每次调用的能力检查

描述符里的 `capabilities` 是逐工具的授权面：一个包可以注册十个工具，只有一个声明 `workspace.write`。调用时：

1. 从参数解析作用域（`path`、`url`、`secret`）；
2. 对描述符里的每个能力向策略请求；
3. 每个决策写一条审计；
4. 全部通过后才调用运行时。

因此"mock 能跑、真机被拒"是正常的：`mock` 不查策略。

### 5.4 结果形状

- 返回 JSON 值：作为工具结果；
- 返回 UI 文档（1.0/1.1 的 `schema_version + root`）：面板动作把它当作下一帧；
- 返回其他 JSON：显示为卡片下的一条消息；
- 错误（JSON-RPC error 或运行时错误）：成为工具错误，会话继续；
- 输出超过 `max_output_bytes`：截断并报告。

### 5.5 schema 建议

- 顶层 `object`；`required` 明确；`additionalProperties: false`；
- 参数名与作用域字段一致（`path`/`url`/`secret`），否则策略提取不到作用域；
- 描述写"做什么/何时用/失败怎样"，不要复述主机策略；
- 工具数量多时依赖延迟暴露；需要模型直接看到的，在 prompt 里点名。

## 6. 命令、提示、上下文、技能与指令

### 6.1 命令

```markdown
---
description: Greet the reader and name the extension
---

Say hello from com.example.hello and name the open workspace.
```

`description` 必填，UI 用它；正文逐字注入；命令 id 是贡献 id（`greet`、`git.refresh`）。命令不执行代码，只组装一条消息（§4.3）。面板动作可以引用命令，也可以调用运行时实现的命令。

### 6.2 提示片段

```markdown
---
description: Hello prompt fragment
---

When the reader asks what this extension does, say that it is the BSH hello example.
```

不由用户调用；宿主按稳定顺序拼接已 apply 包的片段。每轮都付费，保持短、事实性、不重复工具描述、不含秘密。

### 6.3 上下文

```markdown
---
description: all-in-one demo scoped workspace context
paths: crates/**/*.rs, src/**/*.rs
---

This workspace contains Rust crates.
```

`paths` 是何时适用的提示；没有 `paths` 则作用于整个工作区。选择由宿主的 context builder 做，不由模型做。

### 6.4 技能

```text
skills/helper/
  SKILL.md
  references/schema.md
```

```markdown
---
name: helper
description: Walk the reader through the BSH extension framework
version: 1.0.0
---

# Helper
```

校验：`name`/`description` 必填；目录名等于 `name`；正文不得含绝对路径；目录被复制并计入包哈希。frontmatter 是目录条目，正文按需由 `load_skill` 加载。技能正文应写清工具调用、顺序、失败处理；只复述 README 的技能只增加提示成本。

### 6.5 指令与 skill 的进入方式

- prompt/context 进入系统提示的指令集；
- skill 进入技能目录，正文按需加载；
- 宿主发布 `instruction.build` 事件，说明哪个包贡献了什么；
- 事件与钩子不是同一条路径（§10）。

## 7. 描述性贡献

### 7.1 主题

```json
{
  "name": "Midnight",
  "colors": {
    "page": "#0b0d10",
    "text": "#e6e8eb",
    "muted": "#9aa0a6",
    "accent": "#4176e6",
    "border": "#2a2f36",
    "new-session-fill": "#171a1f"
  }
}
```

已知 token：`page`、`text`、`muted`、`faint`、`border`、`border-soft`、`accent`、`error`、`success`、`warn`、`new-session-fill`、`hover`。缺失 token 回退宿主 palette。主题不是 CSS：不能加规则、字体、动画，只做 token 映射。包里第一个主题自动用于该包面板；设置里也可全局选择。

### 7.2 图标与图片

- 面板 `icon`：`asset/icon.svg`；
- 图片节点 `src`：`asset/...`；
- 必须在包内；SVG 当图片画，不当脚本执行；本构建拒绝远程 URL；
- 资源进入不可变版本目录并计入哈希。只有被面板引用的缺失资源才是校验错误。

### 7.3 语言服务

```json
{
  "name": "Rust",
  "extensions": ["rs"],
  "tool": "diagnostics",
  "syntax": "rust"
}
```

`tool` 必须是同包声明的工具。宿主据此回答"哪个语言拥有此文件"，并调用该工具做诊断。CLI：`bsh ext languages`、`bsh ext language-check <id> <lang> <path>`。

### 7.4 外部集成

```json
{ "name": "GitHub", "kind": "github", "action": "issues" }
```

kind ∈ `github | browser | terminal | custom`；`action` 必须是同包声明的工具或命令。加载时校验；引用缺失即包错误，不会出现空菜单。集成是发现面，不是新传输：调用仍走同一运行时、能力、审计路径。CLI：`bsh ext integrations`、`bsh ext integrate <id> <integration> [json]`。

### 7.5 模型提供者

```json
{
  "name": "All-in-One Example",
  "base_url": "https://example.invalid/v1",
  "api_mode": "chat",
  "requires_key": false,
  "demo-model": {
    "model": "demo-model",
    "name": "Demo Model",
    "max_tokens": 4096,
    "input_types": ["text"]
  }
}
```

顶层描述端点；其余每个键是一个模型昵称及其描述。`requires_key = false` 声明开放 lane；缺省或 true 按带密钥处理（安全默认）。无密钥描述让构建不发送凭据，而不是制造"缺 key"拒绝。限制：没有代码级适配器；不能在调用时改写 base URL；模型没有单独能力授予——包级能力就是授予；模型提供者不能直接收钩子。

### 7.6 Agent 角色与预设

```json
{
  "name": "All-in-One Helper",
  "description": "A read-only agent role",
  "plugins": ["files"],
  "permission": "read_only",
  "level": 1
}
```

预设把 provider/model/agent 选择打包给设置层，它不是运行时对象。设置按包与描述符名字列出；合法但未被选择的描述符是惰性的；两个包用了同一昵称必须由设置层消歧，不要依赖顺序。CLI：`bsh ext providers`。

### 7.7 工作流

```json
{
  "description": "Inspect the demo",
  "input_schema": {
    "type": "object",
    "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 20 } },
    "additionalProperties": false
  },
  "steps": [
    { "name": "about", "tool": "describe", "arguments": {} },
    { "name": "log", "tool": "events", "arguments": { "limit": "{{input.limit}}" } }
  ]
}
```

步骤按序；参数可引用 `{{input.name}}` 与 `{{steps.name.field}}`（先前步骤输出）；首个失败步骤使整个工作流失败，没有可选步骤；结果按 `name` 汇总；只能调用同包工具。没有循环/条件/任意代码——需要就写 process 工具。校验：`bsh ext test`；可跑子进程的环境再 `bsh ext mock`。

## 8. 面板与动态 UI

面板是 **JSON，不是 HTML**。宿主拥有控件、焦点、布局、无障碍与主题；包只拥有文档。这是面板可安全渲染、可低成本审查的原因。

### 8.1 面板文件（schema 1.0）

```json
{
  "title": "LLM Sniffer",
  "location": "right",
  "default_visibility": "visible",
  "theme": "midnight",
  "theme_dark": "midnight-dark",
  "refresh_ms": 1500,
  "view": { "schema_version": "1.0", "root": { "type": "column", "children": [] } }
}
```

| 字段 | 默认 | 规则 |
|---|---|---|
| `title` | — | 1..120 字符 |
| `icon` | 无 | `asset/...` |
| `location` | `right` | right / left / bottom / center |
| `default_visibility` | `visible` | visible / hidden |
| `theme` / `theme_dark` | 无 | 必须是同包主题 |
| `refresh_ms` | 无 | 250..60000；可见时按间隔重读 |
| `view` | — | 1.0 或 1.1 文档（§8.4） |

### 8.2 schema 1.0 节点

| 组件 | 形状 |
|---|---|
| container / column / row | `children` |
| text / heading | `text`，heading 有 `level` 1-6，可选 `tone` |
| markdown | `text`（按 markdown 渲染） |
| button | `label`、`action`、`style`（primary/secondary/ghost/danger） |
| input | `label`、`value`、`placeholder`、`action`（真实可编辑） |
| select | `label`、`value`、options `[{label,value}]`、`action` |
| checkbox | `label`、`checked`、`action` |
| list | items `[{title,detail,action?}]` |
| table | `columns`、`rows`，可选 `widths`（每列一个相对权重） |
| tree | `nodes` |
| tabs | `tabs` `[{id,label,content}]` |
| section | `title`、`open`（起始状态）、`children`，读者的开合会被记住 |
| progress | `value` 0-1、`label` |
| code | `language`、`value`，可选 `tone` |
| image | `src` 必须 `asset/...`；远程 URL 拒绝 |
| notification | `level`（info/success/warning/error）、`text` |
| divider / spacer | — |

`tone` 是语义角色而非颜色：`neutral | muted | faint | accent | success | warning | error`。宿主按面板 palette 解析，跟随明暗主题与包主题 token。

**结构限额**：≤1000 节点、深度 32、单文本叶 16 KiB、文本总量 256 KiB、表格 500 行、单元格 2048 字符、tabs 16、select 200 选项、序列化 1 MiB。不能兑现的组件是加载错误，不是静默回退。

### 8.3 动作、修订与刷新

- `action` 是包内贡献 id；按下时先找同名工具，找不到再走命令 seam。
- input 发送 `{"value": ...}`；select 发送选项值；checkbox 发送 `{"checked": bool}`。
- 每个动作携带**渲染时的 revision**；revision 不符即拒绝，旧按钮不能作用到新帧。
- handler 返回 UI 文档：下一 revision 替换卡片；返回其他 JSON：显示为卡片下消息。
- `refresh_ms` 在可见时重读；计时器从渲染起算，隐藏面板自动停止。
- 声明视图是**占位**：先画，再按选择/Refresh 请求 live card；失败时保留声明视图并说明原因。

### 8.4 schema 1.1：surface 面板（4.x）

`panels/<id>.json` 的 `view` 可以是 1.1：

```json
{
  "schema_version": "1.1",
  "bindings": {
    "refresh": { "capability": "acme.git/refresh", "input": { "repo": "$props.repo" } },
    "open": { "capability": "acme.git/open_file", "input": { "path": "$event.value" }, "required": true }
  },
  "root": {
    "type": "column",
    "children": [
      { "type": "button", "id": "refresh", "label": "Refresh", "action": "refresh" },
      { "type": "slot", "surface": "bsh.panel/com.example.diff", "version": "^1", "required": true,
        "sizing": { "weight": 1, "min": 180 }, "fallback": { "type": "text", "text": "waiting" } }
    ]
  }
}
```

与 1.0 的差异：

1. **稳定节点 id**：交互节点（button/input/select/checkbox）**必须**有非空 id；文档内唯一；补丁与焦点用它。
2. **绑定表**：控件 `action` 是绑定键；表在文档级；每个键映射一个 `provider/name` 能力。
3. **slot 节点**：挂载另一个 surface（另一个面板），由宿主解析与展开。
4. 绑定表让一个 1.1 面板的每个动作在**装载时**就被声明成契约动作（risk 取 medium，confirmation 继承）。没有工具实现该能力时，按下会以"capability 没有对应工具"拒绝，而不是静默无效。

**ActionBinding 字段**

| 字段 | 默认 | 规则 |
|---|---|---|
| `capability` | — | `provider/name` |
| `operation` | 无 | 可选操作名 |
| `input` | `{}` | 只允许字面量与映射形式 |
| `principal` | `producer` | `producer`（包/Agent 会话）或 `reader`（宿主会话） |
| `required` | `true` | 解析不到时禁用而不是按下才失败 |
| `mode` | `once` | once 阻止重复派发；repeat 允许 |
| `debounce_ms` | 0 | 0..500，change 事件 |

映射形式只有 `$event.value`、`$event.checked`、`$event.option`、`$props.<name>`、JSON 字面量；以 `$` 开头但不匹配的形式被拒绝；需要 `$` 字面量用 `{ "$text": "$..." }`。

**Slot 字段**

| 字段 | 默认 | 规则 |
|---|---|---|
| `surface` | — | `provider/name`（面板用 `bsh.panel/<panel id>`） |
| `version` | 任意 | semver 范围 |
| `required` | `true` | true 时未解析即 Pending（宿主画骨架）；false 时可画 fallback |
| `props` | `{}` | 必须是对象；子契约声明 schema 时按 schema |
| `sizing.weight` | 1.0 | 0.1..20 |
| `sizing.min/max` | 0/0 | px；max=0 表示不限 |
| `sizing.overflow` | `scroll` | scroll / clip / fit |
| `fallback` | 无 | optional slot 必填 |

**Surface 限额（宿主默认）**：1000 节点 / 深度 32 / 单叶 16 KiB / 总文本 256 KiB / 1 MiB 序列化 / 64 绑定 / 补丁 256 操作 / 补丁 64 KiB / slot 深度 4 / 单节点 slot 扇出 8 / 每 scope 32 实例。

**宿主行为**

- 装载：校验 1.1 文档；把绑定表投影为契约动作；挂载原始 surface（保留绑定与 slot）；渲染器画降级的 1.0 形态。
- 组合：slot 按契约 id 在已挂载面板中解析；展开受深度、节点、字节上限约束；缺失或成环时用 fallback；循环链以面板 id 判定。
- 动作路由：展开时给子控件动作加前缀 `<子面板 id>\u{1f}`；按下时逐段剥前缀，切到该子面板的工具、策略、信任；父面板的 revision 门仍生效。
- 绑定解析：先把动作名当工具 id；否则查绑定表的 capability，按精确 id、`provider/name` 或本地名（含 `namespace__name`）找工具；仍无则拒绝并点名 capability。
- 1.1 面板的 `view` 在运行时可返回 live card；返回的文档同样按 1.1 校验。

### 8.5 宿主槽位（4.x）

| 槽位 | 占用 | 接受 Agent | 仅第一方 |
|---|---|---|---|
| `workspace.main` | single | 是 | 否 |
| `workspace.left` | stacked | 否 | 否 |
| `workspace.right` | stacked | 否 | 否 |
| `workspace.bottom` | stacked | 否 | 否 |
| `workspace.statusbar.leading` | inline | 否 | 否 |
| `workspace.statusbar.trailing` | inline | 否 | 否 |
| `transcript.inline` | inline | 是 | 否 |
| `composer.row` | inline | 否 | 否 |
| `settings.page` | single | 否 | **是** |
| `command.palette.row` | inline | 否 | **是** |
| `empty.hero` | stacked | 是 | 否 |

single 槽位的第二个 presenter 在装载时被拒绝，并记录占用者；stacked/inline 按顺序容纳。设置页与命令面板是宿主 chrome，只能第一方填充。composer/statusbar/palette 目前只有槽位声明，没有包或插件的生产者（§21）。

### 8.6 Agent 自建界面（4.x）

Agent 工具（产物面板由宿主渲染）：

| 工具 | 输入 | 语义 |
|---|---|---|
| `present_surface` | title、slot、document | 校验并呈现一个 1.1 surface |
| `patch_surface` | id、base、ops | 基于 revision 的补丁；陈旧 base 拒绝 |
| `close_surface` | id | 关闭 |
| `surface_catalogue` | 无 | 返回节点词表、限额、可用槽位 |

限额：≤120 节点、≤16 KiB、最多 3 个并存；校验用 agent 限额；标题前显示 Agent 徽标。**动作只读**：面板工具表为空，按下不会调用能力（当前限制，见 §21）。surface 存于进程全局，关闭或重启消失；尚无 turn/session 自动回收。

### 8.7 Surface 总线消息（4.x）

本地总线主题 `surface.message`，载荷是带版本的字节串：

- `frame`：完整文档或 base-relative 补丁；
- `close`：按 instance 关闭；
- `action`：**拒绝**从传输层直接执行，必须走宿主授权管线。

上限 2 MiB；解码校验 wire version；远端帧只允许绑定它自己节点的能力（`validate_remote`）。联邦主题尚未接线（§19）。

## 9. 事件

事件是**已经发生的事实**。有主题的人可订阅；发布者不阻塞；队列满则丢弃并计数。

### 9.1 信封

```json
{
  "id": "uuid",
  "source": "com.example.package",
  "event_type": "tool.completed",
  "schema_version": "1.0",
  "timestamp": 1790000000000,
  "correlation_id": "turn-1",
  "payload": {}
}
```

| 字段 | 含义 |
|---|---|
| `id` | 事件唯一 id |
| `source` | 扩展 id 或宿主 source |
| `event_type` | 主题 |
| `schema_version` | 载荷 schema 代 |
| `timestamp` | Unix 毫秒 |
| `correlation_id` | 所属 turn/call（可知时） |
| `payload` | JSON 对象 |

### 9.2 保留主题

`app.started`、`app.shutdown`、`workspace.opened`、`workspace.closed`、`workspace.changed`、`agent.started`、`agent.completed`、`agent.failed`、`model.requested`、`model.completed`、`config.changed`、`tool.invoked`、`tool.completed`、`tool.failed`、`extension.loaded`、`extension.unloaded`、`extension.failed`、`session.created`、`session.updated`、`session.deleted`、`trajectory.recorded`、`skill.loaded`、`instruction.build`、`network.request`、`mcp.connected`、`audio.captured`、`ui.action`。

主题精确匹配；订阅允许尾部命名空间通配（`tool.*` 匹配 `tool.completed`，不匹配 `tool`）；发布者不能通配。

### 9.3 声明与投递

```toml
[capabilities]
required = ["events.subscribe", "events.publish"]

[contributes]
events = ["workspace.changed", "tool.*"]
publishes = ["git.changed"]
```

| 运行时 | 订阅方式 | 投递 |
|---|---|---|
| process | 宿主按 `contributes.events` 订阅，子进程不发 subscribe | `events/event` 通知 |
| MCP | 宿主按声明订阅，server 启动后投递 | `bsh/events/event` |
| WASM | guest 调 host `subscribe(topic)`，主题必须已声明 | `on-event` |
| Extism | guest 调 `bsh_host` 的 `subscribe` | `bsh_event` |

### 9.4 配额

| 界 | 值 |
|---|---:|
| 订阅者队列 | 默认 64，最大 4096 |
| 事件大小 | 256 KiB |
| 发布速率 | 200/s 每 source |
| 溢出 | 丢弃 + 计数，永不阻塞 |

慢订阅者不拖慢发布者；要每条都收，就必须自己排空队列，否则少订阅。

### 9.5 UI 动作也是事件

面板按钮派发时，宿主路由到所属运行时并发布 `ui.action`（扩展、面板、动作、渲染 revision）。revision 不匹配即拒绝。

## 10. 观测钩子

钩子是**同步 seam 的只读观测**：guest 不能拒绝或替换调用。只有 `extism` 与 `wasm-component` 可声明，manifest 必须带 `hooks.observe`。

### 10.1 观测点

| 点 | 触发 | 事件字段 |
|---|---|---|
| `turn.start` | 用户输入开启一轮 | `session, agent, input_chars` |
| `turn.end` | 轮次落定（成功或失败） | `session, agent, failed, events` |
| `model.request` | 每次 LLM step 前 | `agent, data:{model, request_id, request}`（request 是 JSON 字符串，host 上限 8 KiB） |
| `model.response` | 每次 LLM step 后 | `agent, data:{request_id, response_type, response}` 或流字段 |
| `tool.before` | 每次工具调用前（含命名空间 MCP 工具、`load_skill`） | `agent, data:{tool_name, call_id, tool_args}` |
| `tool.after` | 每次工具调用后 | `agent, data:{tool_name, call_id, tool_result}` |
| `skill.load` | 技能正文附到本轮提示 | `skill, bytes` |
| `http.exchange` | 每次 LLM vendor 调用；沙箱运行时经 bridge 的 HTTP | 元数据、脱敏头、受限正文 |

### 10.2 载荷要点

- `model.request` 的 `request` 是序列化 JSON：每消息预览 512 字符，最多最后 48 条；**没有完整提示**。
- `model.response` 完整路径含 `response`；流路径是累积文本，不是逐 token。
- `tool_args`/`tool_result` 是 JSON 字符串，结果 host 侧上限 8 KiB。
- `http.exchange` 不包 `data`：source/request_id/method/host/path/status/字节数/content_type/TTFB/总时长/error、请求与响应头（secret 头显示 `[redacted]`）、body（各 32 KiB，`body_truncated` 标记）。

### 10.3 投递保证

- 不阻塞 seam：`observe` 入有界队列即返回；
- 批量：每次 guest 调用最多 64 条，同一 point 一批；
- 队列 1024；溢出丢弃并计数；
- 脱敏：`sk-`、`ghp_`、`Bearer ` 被遮蔽；单字段 8 KiB；
- 隔离：一个包失败不影响下一个，也不影响生产者；
- 调 guest 时不持宿主锁；worker 空转 sleep 50 ms。

不保证：投递、跨点顺序、恰好一次、正文（vendor tap 有，WASM/Extism fetch 只有形状）、process/MCP 子进程自开的 HTTP 不可见。MCP 的工具调用可见；当一轮工具数超过 12 个走 MCP gateway 时，看到的是 gateway 本身（`tool_name = "mcp"`，参数里带 `{server, tool, arguments}`），MCP wire 本身不可见。

### 10.4 guest 契约

一次调用一个批次：

```json
{
  "point": "model.request",
  "batch": [
    { "agent": "assistant", "data": { "...": "..." } }
  ]
}
```

guest 应解析 envelope、遍历 batch、逐条做有界工作、返回合法 JSON（如 `{"action":"continue"}`；observe 忽略 action）。返回空串/非法 JSON 使 call 失败并记录日志。

### 10.5 交付路径

| 运行时 | 入口 |
|---|---|
| Extism | 导出 `bsh_hook` |
| WASM component | `on-event`，`event-type = "hook"`，载荷 `{point, batch}` |

只投给**已 apply** 且声明了该点的包；声明了但没导出入口会在投递时记录运行时错误，不假装已挂上。unapplied（缺授权、manual、依赖失败）收不到，因为不在目标列表。

## 11. 运行时全表与逐运行时

### 11.1 选择表

| 运行时 | 隔离 | 代码 | 钩子 | 存储/网络 | 动态面板 | 典型用途 |
|---|---|---|---|---|---|---|
| builtin | 进程内（高信任） | Rust，链接进 BSH | 否 | 宿主直接 | 是 | 第一方能力 |
| declarative | 进程内，零代码 | 无 | 否 | 无 | 静态/声明式 | echo/常量/模板、脚手架 |
| process | 子进程（非 OS 沙箱） | Python/Node/shell/编译程序 | 否 | 自己的 fs/网络；宿主可 mediation | 是（panel 方法） | 已有程序 |
| mcp | 子进程 + MCP | MCP server | 否 | 自己的 | resources→contexts | 现成 MCP 生态 |
| extism | WASM + PDK，WASI 关闭 | 多语言 | **是** | 只能经 bridge | 是（bsh_panel） | 可移植插件、观测 |
| wasm-component | WASM Component，最严 | WIT 绑定 | **是** | 只能经 host imports | 是（panel 导出） | 高隔离、可移植 |
| cordis | Node 子进程 | JS apply(ctx,config) | 是（hook 注册） | 子进程 fs；宿主服务多被拒 | 经 slot/panel 注册 | DSH/Cordis 包 |

### 11.2 builtin

按 id 注册的 Rust 运行时，随 BSH 链接；用于第一方能力。没有 manifest 文件约定外的载荷；不经过子进程；信任级别最高。

### 11.3 declarative

零代码路径：宿主执行描述符里的三种纯变换。适用 echo/常量/模板、想先立骨架、希望"运行时不可能失败"。不适用需要文件、网络、存储、秘密、时间、调用其他工具、进度、事件、动态面板的场景。没有钩子、没有宿主调用、没有代码执行——这是特性，不是缺口；它也是最容易审查的包。声明但实现从不使用的能力在审查中是警示。

### 11.4 process

**包**：`extension.toml` + `server.py` + `tools/echo.json`；manifest 的 `runtime.kind = "process"`，`entry`、`args`、`protocol = "jsonrpc-2.0"`、`startup`。

**协议**：stdin/stdout 每行一个 JSON 对象。

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1.0","extensionId":"com.example.x","host":"bsh","token":"per-session-nonce"}}
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0","token":"per-session-nonce"}}
```

握手强制：

1. 宿主发送 initialize（protocolVersion、extensionId、host、token）；
2. server 返回以 `1.` 开头的 protocolVersion 与同一 token；
3. token 同时以 `BSH_IPC_TOKEN` 环境变量给无法解析 stdin 的启动器。

缺 token 或不匹配是硬拒绝（防止继承管道的子进程应答另一个宿主）。方法：initialize、tools/call、shutdown；可发 `$/progress` 通知。shutdown 应在 1 秒内应答。

**错误**：应用级错误用 JSON-RPC error（如 `{"code":-32000,"message":"..."}`），变成工具错误且会话可用；传输失败（stdout EOF、超时、不可解析行）杀死会话，下一次调用启新子进程，不读陈旧响应。未知 id 的响应按迟到答案跳过；超时调用杀死子进程；stderr 只作诊断日志。

**限额**：单行 4 MiB；单结果 `max_output_bytes`；单调用 min(调用方 deadline, `max_execution_ms`)；握手 10s；一个子进程、调用串行（`max_concurrent_calls` 是声明额度，不是池化）。

**宿主强制**：环境清空后只加白名单与 manifest 变量；cwd 为包根；可执行文件必须包内相对或白名单启动器；需要 `process.execute`（可按程序/参数限定）；进程组与 RLIMIT；macOS 仅文件写沙箱；Linux 的 process/MCP 默认启用 bubblewrap，缺 bubblewrap 或 namespace 失败即 fail closed，绝不无沙箱回退；`--share-net` 意味着子进程网络仍共享；stdout/stderr 截断。

### 11.5 WASM component

**契约**（`crates/bsh/wit/extension.wit`）：world `bsh-extension` 导入 `host`、导出 `extension`。导出调用顺序：`initialize → activate → tools → call-tool → command → panel → on-event → deactivate → shutdown`。

| host 函数 | 能力 |
|---|---|
| read-file / write-file | workspace.read / workspace.write |
| fetch | network.http |
| storage-get / storage-set / storage-list | extension.storage |
| read-secret | secrets.read |
| emit / subscribe | events.publish / events.subscribe |
| configuration | 无（返回已校验配置） |
| now-millis / progress / log | 无 |

每个函数返回 `result<_, host-error>`，错误为 `denied | not-found | limited | runtime | timeout`。

**manifest**：`kind = "wasm-component"`，`entry` 为包内 `.wasm`；激活时编译，坏组件使包失败而非宿主。

**限额**：`max_memory_mb` 经 StoreLimits；`max_execution_ms` 经 fuel/epoch；结果 `max_output_bytes`。取消在下一个 host 调用处 trap；纯计算 guest 可能跑到 fuel 期限。WASI preview 2 提供 clock/random 与继承的 stdout/stderr；stdin、文件系统、环境变量不提供，无 socket 权限。

### 11.6 Extism

**导出**：`bsh_tools`、`bsh_call_tool`、`bsh_command`、`bsh_panel`、`bsh_event`、`bsh_hook`（声明了 hook 才需要；声明却没导出会在投递时记录运行时错误）。

**bridge envelope**：每次 `bsh_host` 调用一个 JSON 请求、一个 JSON 回复：

```json
{"ok":true,"value":...}
```

或

```json
{"ok":false,"error":"..."}
```

成功必须解 `value`；`ok:false` 当错误。把回复当裸值读是本运行时最常见的 bug。

**操作**：`read_file/write_file`（workspace.*，包或工作区相对且受限）、`fetch`（network.http，正文有上限）、`storage_get/set/list`（extension.storage，仅自己的 namespace）、`read_secret`（secrets.read）、`emit`/`subscribe`（声明过的主题）、`config`/`now_millis`/`log`（无能力）。WASI 禁用，bridge 是唯一文件/网络路径。

**限额**：单 bridge body 8 MiB；结果 `max_output_bytes`；内存 `max_memory_mb`；`fetch` 只返回文本或 base64，**没有状态码/头**（那些进 `http.exchange`）；guest 调用持插件锁，工具调用期间的 hook 在该调用结束后才跑；hook 失败只记日志，不影响其他包。

### 11.7 MCP

manifest：`kind = "mcp"`，`entry`/`args`/`protocol = "mcp"`；仍需 `process.execute`，MCP 不绕过进程策略。

握手：启动 server → `initialize`（协议版本 + client info）→ `notifications/initialized` → `tools/list`、`resources/list`、`prompts/list` → 映射：tools→BSH 工具；resources→contexts（`resources/read`）；prompts→prompt 贡献（`prompts/get`）。调用走 `tools/call`；progress 转发；错误变工具错误；EOF/超时杀死会话，下一次调用启新 server。

声明的贡献必须真的返回；声明却缺失是加载错误，不是空面。事件：声明 `contributes.events` 的包在 server 运行后收到 `bsh/events/event`；宿主尚未启动 server 时丢弃排队事件，而不是为事件启动它。

限额：一个 server、调用串行；超时与输出跟随 process；HTTP 请求不被宿主捕获；诊断必须写 stderr，stdout 写诊断会破坏协议。

### 11.8 Cordis（JS）

`kind = "cordis"`，`entry` 是 JS 文件，以 `apply(ctx, config)` 在既有能力门控的 Node bridge 子进程中运行。bridge 把注册物映射为 BSH 贡献：

| 注册 | 贡献 |
|---|---|
| `ctx.tools.register` | tool |
| `ctx.commands.register` | command |
| `ctx.skills.register` | skill（物化到 data 目录） |
| hook 点 | hook |

协议帧：`load`、`invoke`（`{registration, input}`）、`register`（`{registration, kind, ...}`）、`refuse`、log。宿主服务多数被显式拒绝并在错误里点名：`ctx.fs`、`ctx.subprocess`、`ctx.llm`、`ctx.settings`、`ctx.credentials`、`ctx.slots`（当前无 DSH 渲染树）、`ctx.loader`、`ctx.fiber`、`ctx.reflect`、`ctx.ptcRuntime`、`ctx.session(s)`。这是安全边界：插件不获得宿主文件系统、模型或凭据。

注意：这里的 "Cordis" 是 **JS 包适配器**；Rust 的 cordis-rs 作为 surface provider 生命周期宿主是另一件事，代码在 `crates/ferris-runtime/src/surface_runtime.rs`。

### 11.9 启动策略

| 策略 | 何时应用 |
|---|---|
| `on-startup` | 宿主加载后立即预热（分离线程，不阻塞构建） |
| `on-demand` | 先注册描述符；首次调用才启动 |
| `on-workspace-open` | 工作区打开时应用 |
| `manual` | 从不自动；读者手动 |

process 握手 10 秒；超时即 startup error。

## 12. 生命周期与激活

### 12.1 状态机

```text
Discovered -> Validating -> Resolved -> Installing -> Installed
Installed -> Loading -> Starting -> Active
Active -> Suspending -> Suspended -> Starting
Active -> Stopping -> Unloaded
any non-Unloaded -> Failed
Failed -> Loading (retry)
Unloaded -> Loading (reinstall)
```

未定义转移是错误，不是静默改写；失败总是合法——目的是停下坏包。

| 状态 | 含义 |
|---|---|
| discovered | 看到目录，未校验 |
| validating | manifest/贡献校验中 |
| resolved | 依赖已选 |
| installing | 暂存、哈希、验签 |
| installed | 字节在仓 |
| loading | 构建运行时 |
| starting | 握手/guest 启动（10s） |
| active | 已授权并加载 |
| suspended | 被宿主暂停 |
| failed | 记录最后错误 |
| unloaded | 停止或禁用 |

### 12.2 enabled / active / applied / configured

- **enabled**：`state.toml` 里的读者开关；
- **active**：生命周期到达 Active（授权并加载）；
- **applied**：贡献已装入本次构建的 agent；
- **configured**：配置 schema 通过且默认值已应用。

常见组合：active 但 applied=no（能力被拒、manual、依赖失败）。`bsh ext info` 同时打印四个，不用猜。

### 12.3 失败、重试、隔离

- 失败写 `state.toml` 的最后错误与状态；
- `bsh ext retry <id>` 清除持久失败并重载；
- `bsh ext quarantine <id> --reason ...` 把代码移出 installed 并禁用——对"不再信任"的包，不是普通 disable；
- `bsh ext rollback <id>` 激活上一版本；
- 激活失败会回滚已启动的集合；manager 同时持有运行时与贡献租约，卸载时统一撤销。

## 13. 能力、授权、审计与网络出口

### 13.1 能力词表

| 能力 | 风险 | 含义 |
|---|---|---|
| workspace.read | low | 读工作区文件 |
| workspace.write | high | 写/建/删工作区文件 |
| network.http | high | 出站 HTTP |
| process.execute | critical | 启动/管理外部进程 |
| computer.use | critical | 读屏/驱动键鼠 |
| secrets.read | critical | 读取命名秘密 |
| extension.storage | low | 扩展私有存储 |
| agent.tools.register | low | 注册模型工具 |
| agent.context.provide | low | 提供作用域上下文 |
| agent.prompts.provide | low | 提供提示片段 |
| agent.skills.provide | medium | 提供技能 |
| agent.models.provide | medium | 注册模型/提供者 |
| agent.agents.provide | medium | 注册 Agent 角色 |
| agent.workflows.provide | medium | 注册工作流 |
| events.publish | low | 发布事件 |
| events.subscribe | low | 订阅事件 |
| ui.panels | low | 贡献面板 |
| ui.commands | low | 贡献命令/快捷键 |
| ui.theme | low | 贡献主题 |
| hooks.observe | high | 收观测批次 |
| agent.intercept | high | 拦截/改写宿主事件（**尚未实现**） |

`terminal.execute` 是 `process.execute` 的别名。

### 13.2 授权模型

- 授予按 **能力 + 作用域** 记录；声明不是授予；
- 每次调用都检查，包括扩展发起的服务调用；
- 作用域从参数提取：`path` → 工作区、`url` → 域、`secret` → 名字；
- 撤销立即阻止新调用；在途调用按策略 drain/cancel；
- 热更新重新评估授予，不静默继承旧产物摘要的权限；
- 签名证明完整性与发布者身份，不证明安全；
- 原生进程内扩展是高信任执行类，不是沙箱；process/WASM/Extism/MCP 各自声明隔离与取消语义；
- 策略引擎、宿主服务、沙箱或信任根不可用时 **fail closed**；
- 审计记录：allow/deny、principal、capability、resource、scope、extension、activation、generation、provider、correlation、policy version。

### 13.3 网络出口

`network.http` 是公共出口授予。每一跳（初始 URL 与每次重定向）解析一次，**所有**返回地址都分类，已验证地址 pin 进客户端，防 DNS rebinding。默认拒绝 loopback、私有、link-local、CGNAT、multicast、unspecified、broadcast、documentation、benchmark 段，包括"公网+私网混合答案"的主机名。唯一例外是授予里写死字面 IP（如 `domain:127.0.0.1` 的本地测试），那是另一类地址作用域私网授予。TLS 校验与 Host 头仍用 URL 主机名。

## 14. 存储、配置、服务与依赖

### 14.1 存储

每个扩展拥有 `extension-data/<id>/`。键是相对路径；绝对路径、`..`、`.`、反斜杠拒绝；规范父目录必须留在 namespace 内（抓 symlink）；值不能是 symlink。

| 界 | 值 |
|---|---:|
| 单值 | 8 MiB |
| 每 namespace 文件数 | 10,000 |
| namespace 总量 | 64 MiB |
| 写入 | 原子（temp + fsync + rename） |

WASM/Extism 经宿主调用访问；process/MCP 除非宿主提供 surface，否则直接访问不到自己的 namespace。存储用于业务数据，不用于可重建缓存或代码；升级保留，卸载默认保留，`--purge-data` 才删。

### 14.2 配置

```toml
[configuration]
schema = "config.schema.json"
defaults = { "remote" = "origin" }
```

宿主按 schema 校验读者配置并把结果交给运行时：process → `initialize.params.config`；WASM → `configuration()`；Extism → bridge `config`；MCP → `initialize.params.config`（额外字段，server 可忽略，不要依赖它做必填设置）。配置错误是加载错误；`ext info` 的 configured 字段说明是否通过。

CLI：`bsh ext config <id>`、`... schema`、`... set '<json>'`、`... set config.json`。

### 14.3 服务

提供者声明 name/contract/version/scope/capabilities；消费者声明 name/contract/version/scope/optional。scope ∈ host/workspace/session/extension。解析精确匹配契约与版本；required 不可满足阻塞激活。发现不是授权：消费者仍需持有提供者声明的能力。CLI：`bsh ext graph`、`bsh ext plan`。

### 14.4 依赖与锁

`[dependencies] id = "^1.0"`；环与不满足在激活前拒绝。lock 记录 format、host、extension API，以及每包的 exact version、hash、source 与解析后的依赖版本；排序确定。`bsh ext reproduce <lock>` 精确重装，覆盖 Git/registry 远端与目录安装的本地源路径。

## 15. 限额总表

权威常量在源码；本表用于审查与排障。任何一项都可能在宿主策略里被调低。

| 类别 | 界 | 值 |
|---|---|---:|
| 包 | 归档/下载 | 256 MiB |
| manifest | 文件 | 256 KiB |
| 贡献文件 | 单文件 | 256 KiB |
| 配置 | schema/值 | 256 KiB |
| 面板 1.0 | 节点 / 深度 | 1000 / 32 |
| 面板 1.0 | 单叶 / 总文本 | 16 KiB / 256 KiB |
| 面板 1.0 | 表格行 / 单元格 | 500 / 2048 字符 |
| 面板 1.0 | tabs / select 选项 | 16 / 200 |
| 面板 1.0 | 序列化 | 1 MiB |
| 面板 1.0 | refresh_ms | 250..60000 |
| surface 1.1 | 节点 / 深度 | 1000 / 32 |
| surface 1.1 | 单叶 / 总文本 / 序列化 | 16 KiB / 256 KiB / 1 MiB |
| surface 1.1 | 绑定 / 补丁操作 / 补丁字节 | 64 / 256 / 64 KiB |
| surface 1.1 | slot 深度 / 单点扇出 / 每 scope 实例 | 4 / 8 / 32 |
| surface 1.1 | 传输消息 | 2 MiB |
| Agent surface | 节点 / 字节 / 并存 | 120 / 16 KiB / 3 |
| 存储 | 单值 / 文件数 / namespace | 8 MiB / 10000 / 64 MiB |
| 事件 | 单事件 / 发布速率 | 256 KiB / 200 每秒每 source |
| 事件 | 订阅队列 | 64..4096 |
| 事件 | 跟踪发布者 | 4096 |
| 钩子 | 队列 / 批次 | 1024 / 64 |
| 钩子 | 字段 / body | 8 KiB / 32 KiB |
| 钩子 | worker 空转 | 50 ms |
| process | 单行 / 握手 / shutdown | 4 MiB / 10 s / 1 s |
| Extism | bridge body | 8 MiB |
| 工具 schema | 每轮累计 / 单工具 | 64 KiB / 16 KiB |
| 工具 schema | 每工具上限常量 | 16 KiB（`DEFAULT_PER_TOOL_BUDGET`） |

工具 schema 预算超限只产生告警日志（`schema_budget=used/limit`），不会拒绝调用；它提示"该用 preset allowlist 或懒加载 MCP 了"。

## 16. CLI 全命令

### 16.1 顶层

```text
bsh [--settings[=SECTION]] [--panel=PANE] [--browser=URL] [--federation-profile=NAME]
    ext <command> [options]
    federation [--profile NAME] <command> [options]
```

- `--settings` 打开设置并定位 rail cell 或托管视图；
- `--panel`：files | terminal | diff | browser | extensions | federation | tasks；
- `--browser=http(s)://...` 打开浏览器面板；
- `--federation-profile NAME` 让本实例以独立节点 profile 运行；
- 未知的裸首参现在直接报错并退出 2（拼错 `federaton` 不再静默启动 GUI）；
- 退出码：0 成功、1 失败、2 用法错误。

### 16.2 bsh ext

| 命令 | 作用 |
|---|---|
| `new <id> [--dir]` | 生成 2.0 declarative 脚手架 |
| `validate <path>` | 解析并校验包 |
| `migrate <path> [--write]` | 1.x → 2.0，保留 `extension.toml.v1` |
| `hash <path>` | 打印规范包哈希 |
| `build / package <path> [--out]` | 校验并打 .tar.gz |
| `test <path>` | 校验包与声明的贡献 |
| `mock <path> [tool] [json]` | 用宽松 mock 宿主跑一个工具 |
| `install <path / archive / git+url>` | 安装进仓库 |
| `uninstall <id> [--purge-data]` | 卸载，默认保留数据 |
| `list [--json]` | 已安装与内建 |
| `plan [--json]` | manager 激活顺序 |
| `graph [--json]` | 服务/依赖图 |
| `why <id>` | 解释为何 applied / 未 applied |
| `effects <id>` | 声明的影响面 |
| `reload <id>` | 重建该扩展的宿主视图 |
| `mount <file>` | 应用 Cordis/profile patch |
| `info <id>` | 单个扩展详情 |
| `enable / disable <id>` | 切换激活 |
| `versions <id>` | 已安装版本 |
| `update <path>` | 安装并激活新版本 |
| `rollback <id>` | 激活上一版本 |
| `reproduce <lock>` | 按锁精确重装 |
| `retry <id>` | 清失败并重载 |
| `quarantine <id> [--reason]` | 移出代码并禁用 |
| `grant / revoke <id> <capability>` | 改变授权 |
| `capabilities` | 实时能力词表 |
| `logs <id>` | 审计与运行时日志 |
| `config <id> [set <json 或 file> 或 schema]` | 读/写配置 |
| `metrics` | 调用/事件/订阅计数 |
| `providers / languages / integrations` | 列描述符 |
| `language-check <id> <lang> <path>` | 跑语言诊断 |
| `integrate <id> <integration> [json]` | 跑集成动作 |
| `market [query]` / `market install <id> [version]` | Git 市场 |
| `doctor [--json]` | 仓库、哈希、依赖、策略检查 |
| `keygen / sign / verify / trust / untrust / publishers` | 签名与信任 |
| `registry add/list` | 私有 registry |

全局选项：`--root <path>`、`--json`。

### 16.3 bsh federation

| 命令 | 作用 |
|---|---|
| （无）/ `help` | 帮助 |
| `serve` | 前台运行该 profile 节点；可重复 `--profile` 跑多节点 |
| `pair <profile> [--grant <cap>]...` | 让两个本地 profile 互信（授予对方调用本节点能力） |
| `profiles` | 本机所有节点 profile、状态、peers、node id、name |
| `status` | 本节点 id、peers、最近决策 |
| `enroll / invite / approve / suspend / resume / revoke <node-id>` | 信任生命周期 |
| `grant / revoke-grant <node-id> <capability-id>` | 能力授予 |
| `rotate [reason]` / `accept-rotation <record.json>` | 密钥轮换 |

`--profile NAME` 选择操作哪个节点；`-p` 同义。节点 id 是 base64 Ed25519 公钥。

## 17. 测试、调试与验收

### 17.1 四层测试

| 层 | 命令 | 证明什么 | 不证明什么 |
|---|---|---|---|
| 校验 | `ext validate` | manifest、文件、贡献引用、UI 文档 | 运行、权限 |
| 贡献测试 | `ext test` | 声明的贡献可加载、内容结构 | 真实运行时握手 |
| mock | `ext mock` | 真实运行时的握手与调用形状 | 能力策略、真实工作区 |
| 宿主测试 | `cargo test -p bsh --lib extension::` | 端到端装配、策略、运行时 | 你的业务逻辑 |

`mock` 不查策略：mock 能跑、真机 denied 是正常现象。

### 17.2 验收门（CI）

```bash
cargo fmt --all -- --check
cargo clippy --workspace --exclude jsbos --exclude nbos --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo test -p ferris-core --features tokio
cargo test -p ferris-runtime --features "wasm extism cordis"
cargo test -p ferris-sdk
cargo test -p bsh --lib extension::
scripts/ci/check-extension-gold-standard.sh
scripts/ci/check-ferris-independence.sh
```

测试数量随版本变化，以 CI 输出为准；本手册不固定数字。

### 17.3 症状 → 原因

| 症状 | 常见原因 | 先查 |
|---|---|---|
| 包不出现 | 未 enable、store 未刷新、manifest 校验失败 | `ext list`、`ext info`、日志 |
| active 但 applied=no | 能力被拒、manual、依赖失败 | `ext info`、`ext why` |
| denied | 缺 grant / 作用域不符 | `ext grant`、审计行 |
| 握手失败 | token 未回显、protocolVersion 非 1.x、stdout 混诊断 | server 的 initialize |
| mock 通过、真机失败 | 策略/真实工作区 | 审计 |
| 面板空白 | view 校验失败、live card 返回非文档、revision 过期 | 面板错误行、`panel` 返回 |
| 面板动作无反应 | 1.0 动作 id 不是贡献；1.1 绑定 capability 没有工具 | 日志、绑定表 |
| 1.1 面板加载失败 | 交互节点缺 id、绑定语法错、slot 缺 fallback、超出限额 | `ext validate` 的具体字段 |
| slot 不显示 | 目标 surface 未挂载、成环、超出深度/字节 | inspector rows、refusal |
| 事件收不到 | 未声明主题、未排空队列、运行时启动前丢弃 | `contributes.events`、`ext logs` |
| 钩子收不到 | 未 apply、未声明点、没导出 entry | 运行时错误日志 |
| MCP 工具全丢 | server 往 stdout 写诊断 | 改 stderr |
| Extism 全 null | 没解 `{ok,value}` | bridge 测试 |
| hash mismatch | 手改 installed、签名后改包 | `ext doctor`，重装 |
| 0 capabilities 的 peer | 对方 `federation.expose` 为空或未 grant 本节点 | 对方设置 + `federation grant` |
| 联邦节点未启动 | 同 profile 目录被另一实例锁住 | 换 `--federation-profile` 或关旧实例 |
| 未知命令直接退出 2 | 子命令拼错（如 federaton） | 用 `bsh federation ...` |
| schema 预算告警 | 工具太多、schema 太大 | preset allowlist / 懒加载 MCP |

### 17.4 调试顺序

1. `ext validate`（静态错误最先暴露）；
2. `ext info` 看 enabled/active/applied/configured 与 last_error；
3. `ext why` 看决策；
4. `ext logs` 与审计行（谁、哪个能力、哪个作用域被拒）；
5. 运行时专属：process 看握手/stderr，MCP 看 initialize 结果，Extism 看 envelope，WASM 看 host-error，Cordis 看 refuse 帧；
6. 仍不通，再用 `ext mock` 区分"运行时问题"与"宿主策略问题"。

## 18. 打包、签名、市场与升级

### 18.1 哈希

覆盖相对路径、字节长度、内容摘要，排序；排除 `.git`、`.jj`、`target`、`node_modules`；`package.sig` 不参与它签名的哈希。`ext hash` 打印，`ext doctor` 重算并报不一致。

### 18.2 签名与信任

```bash
bsh ext keygen acme --out /tmp/acme.key
bsh ext sign /tmp/hello-release --publisher acme --key /tmp/acme.key
bsh ext trust acme <public-key>
bsh ext verify /tmp/hello-release
```

签名是 Ed25519，覆盖 `bsh-package-v1`、id、精确版本、包哈希。安装行为：

- 可信发布者的有效签名 → trust `verified`；
- 不可信发布者的有效签名 → 警告，trust 保持 local；
- 无效签名 → 拒绝安装。

哈希证明完整性；只有已验证签名证明"谁做的"。

### 18.3 市场

市场是 Git 仓库，不是服务。官方布局：

```text
bsh_extensions/
  index.json
  packages/<id>/<version>/...
```

`ext market` 列条目；`ext market install <id> [version]` 装最新兼容版；`ext registry add/list` 配私有提供者。一个 index 可广告同一 id 的多个版本；设置里一行一个扩展、行内选版本；index 不再携带的已安装版本仍可回滚。

### 18.4 升级、回滚、隔离

| 动作 | 命令 | 效果 |
|---|---|---|
| 升级 | `ext update <path>` | 装新版本并激活 |
| 回滚 | `ext rollback <id>` | 激活上一版本 |
| 重试 | `ext retry <id>` | 清失败并重载 |
| 隔离 | `ext quarantine <id> --reason ...` | 移出代码并禁用 |
| 卸载 | `ext uninstall <id> [--purge-data]` | 删除，默认保留数据 |

不要用编辑 `installed/` 来"修"坏安装：哈希会与 registry 不一致，`doctor` 会报篡改。

## 19. 联邦与扩展

### 19.1 节点与 profile

- `federation.enabled` 默认 **true**：只应答已登记 peer 的节点在登记与授权前是无害的；
- **一个 profile 是一个节点**：自己的 key、peers、delegations、审计；默认 profile 沿用旧布局（文件直接在 `federation/` 下），其他 profile 在 `federation/<profile>/`；
- 同 profile 只能有一个 writer：`node.lock` 被运行中的节点持有；
- 多开 GUI：`bsh --federation-profile work`（或 `BSH_FEDERATION_PROFILE=work`）；
- 无窗口运行：`bsh federation --profile work serve`；
- TLS 是进程级：一个进程一个 TLS 节点。

### 19.2 发现

- 每 30 秒刷新一轮：对每个已登记 peer 调 `discover`；
- 单个探针 **5 秒**超时（4.x 修正：此前用通用 30 秒回复超时，一个死 peer 会拖慢整轮并顺延下一轮）；
- peers 并发探测，一个死 peer 不阻塞其他 peer 的答案；
- 结果写入 peer catalogue；0 个能力会明确记日志（对方没 expose 或没 grant 本节点）；
- 面板/CLI 的 `status` 可看最近决策。

### 19.3 暴露与授权

节点默认**不暴露任何能力**。可暴露的读取类工具是：

```toml
[federation]
expose = ["builtin/read_file", "builtin/read_files", "builtin/glob", "builtin/grep"]
expose_root = "/path/to/confine"
include_remote_tools = true
remote_work = true
max_task_steps = ...
max_task_tokens = ...
max_task_deadline_ms = ...
```

- `expose` 为空 = peer 看到的 catalogue 为 0；
- `expose_root` 限定被暴露读工具的作用域；
- 授予在**提供方**节点执行：`bsh federation --profile <p> grant <consumer-node-id> builtin/read_file`；
- 消费方 `include_remote_tools = true` 才把远端能力作为工具提供给模型；
- 本地已有同名能力时，本地优先，不被远端遮蔽；
- 远端任务/目标委派受 `remote_work` 与三个预算约束。

### 19.4 故障诊断

| 现象 | 含义 | 处理 |
|---|---|---|
| `another BSH instance is using the federation data directory` | 同 profile 已被占用 | 关旧实例，或换 `--federation-profile` |
| peer 不答发现 | 对端未运行 / endpoint 或 mode 不匹配 / 被 suspend/revoke / 防火墙 | `status`、`profiles`、`revoke/suspend` |
| peer offers 0 capabilities | 对端 expose 为空或未 grant | 对端配置 expose + grant |
| 30 秒日志密集 | 已修复为 5 秒探针 + 并发；若仍见，确认二进制已更新 | 重建/重启 |

### 19.5 Surface 与联邦

动态 UI 的 surface 消息目前只走**本地总线**（`surface.message`：frame/close；action 从传输层拒绝）。远端帧的绑定校验（只允许自己节点的能力）已在框架层实现，但联邦主题、关联与双端审计尚未接线。因此"跨 BSH 的 UI 能力"仍是设计目标，不是当前能力。

## 20. 第一方与内建扩展

### 20.1 扩展优先原则

BSH 的硬规则：**每个新的 BSH 功能都必须作为第一方扩展，走与第三方完全相同的 manifest、能力、生命周期与 effect 路径**。需要新宿主 seam 时，把它暴露为类型化服务或事件；在核心里加特权捷径是例外，必须 RFC + 迁移计划。验收标准是 `docs/bsh-extension.md`；在其 conformance gate 未满足前，不能称框架完成。

### 20.2 builtin 运行时

第一方 Rust 能力用 `runtime.kind = "builtin"`，按 id 注册进宿主；它仍通过同一贡献、租约、能力与审计路径暴露，不绕过扩展平台。内建包与已安装包一起出现在 `ext list` 中。

### 20.3 随仓示例

| 示例 | 演示 |
|---|---|
| `examples/extensions/hello` | 最小 declarative：工具 + 命令 + 面板 |
| `process-echo` | JSON-RPC 子进程、握手、进度 |
| `wasm-echo` | WIT 组件、host imports |
| `extism-echo` | Extism 导出与 bridge envelope |
| `mcp-echo` | MCP tools/resources/prompts 映射 |
| `hook-demo` | 观测钩子与采集 |
| `language-python` | 语言服务描述符 |
| `builtin-template` | builtin 运行时形态 |
| `allinone` | 多贡献组合与工作流 |

## 21. 已知限制与设计边界

- **钩子只观测**：`agent.intercept` 在词表中但未实现；guest 不能拒绝或改写 seam 调用。
- **declarative 无代码**：三种纯变换，没有文件/网络/存储/秘密/时间，没有钩子。
- **没有文件监视热重载**：热重载是宿主驱动的 `ext reload`，不是 watcher。
- **市场是 Git**：没有专有 registry 服务；私有提供者用 `registry add` 配 URL。
- **process/MCP 不是全平台 OS 沙箱**：`process.execute` 是对包的信任决定，不是容器；Linux 默认 bubblewrap 且 fail closed，macOS 只有文件写沙箱；子进程自己的 socket 宿主不可见。
- **原生进程内扩展是高信任类**：实现 Ferris trait 不等于被沙箱。
- **Cordis JS 的 `ctx.slots` 仍被拒绝**：DSH 的 slot API 传的是 JS 渲染函数，无法序列化成 surface 文档；需要新的声明式 bridge 方法，当前没有 DSH 渲染树。Rust 的 cordis-core surface provider runtime 是另一条路径（§11.8 末注）。
- **组件 ABI 仍是 1.0**：WIT 只导出 `panel(id)`；process/WASM guest 还不能声明 surface/patch/close。向既有 world 加导出会让所有 1.0 guest 实例化失败，必须新包 + 双 bindgen。
- **Agent surface 动作只读**：面板工具表为空，绑定 capability 没有 dispatch 路径；surface 存于进程全局，最多 3 个，尚无 turn/session 自动回收。
- **面板 1.1 动作解析到工具**：绑定 capability 必须能匹配到同面板工具（精确 id、`provider/name` 或本地/命名空间名），否则拒绝；`principal = reader` 的宿主动作白名单尚未接线。
- **composer/statusbar/palette 只有槽位**：没有包或插件生产者；设置页与命令面板仅第一方。
- **联邦不承载 surface**：本地 `surface.message` 可用，联邦 topic/关联/双审计未接线。
- **工作流不是语言**：无循环、无条件、无可选步骤；只能调用同包工具。
- **MCP 配置字段**：`initialize.params.config` 是额外字段，server 可忽略，不要用于必填设置。
- **工具 schema 预算**：每轮累计 64 KiB、单工具 16 KiB；超限告警，不拒绝调用。
- **native/process/WASM 的取消语义不同**：WASM 在下一个 host 调用 trap；纯计算 guest 可能跑到 fuel 期限；进程超时会杀子进程。

---

## 附录 A. manifest 模板

```toml
schema_version = "2.0"

[extension]
id = "com.example.pkg"
name = "Package"
version = "1.0.0"
description = "What it does, in one paragraph"

[compatibility]
host = ">=4.0.0, <5.0.0"
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

[configuration]
schema = "schemas/config.schema.json"

[configuration.defaults]
verbose = false

[resources]
max_memory_mb = 256
max_execution_ms = 30000
max_concurrent_calls = 4
max_output_bytes = 1048576

[dependencies]

[services]
provides = []
requires = []
```

## 附录 B. 文件索引

| 路径 | 作用 |
|---|---|
| `extension.toml` | 唯一 manifest |
| `package.sig` | 可选签名 |
| `tools/<id>.json` | 工具描述符 + declarative 体 |
| `commands/<id>.md` | 命令模板（frontmatter: description） |
| `panels/<id>.json` | 面板（1.0 / 1.1 view） |
| `prompts/<id>.md` | 提示片段（description） |
| `contexts/<id>.md` | 上下文（description, paths?） |
| `skills/<id>/SKILL.md` | 技能（name/description/version） |
| `workflows/<id>.json` | 工作流 |
| `themes/<id>.json` | 主题 token |
| `presets/<id>.json` | 预设 |
| `providers/models/<id>.json` | 模型提供者 |
| `providers/agents/<id>.json` | Agent 角色 |
| `languages/<id>.json` | 语言服务 |
| `integrations/<id>.json` | 外部集成 |
| `asset/...` | 图标/图片 |

## 附录 C. 面板 1.0 节点速查

```text
container/column/row  children
text/heading          text, [level 1-6], [tone]
markdown              text
button                label, action, [style]
input                 label, [value], [placeholder], action
select                label, [value], options[{label,value}], action
checkbox              label, [checked], action
list                  items[{title,detail,action?}]
table                 columns, rows, [widths]
tree                  nodes[{id,label,children}]
tabs                  tabs[{id,label,content}]
section               title, [open], children
progress              value 0-1, [label]
code                  [language], value, [tone]
image                 src=asset/..., [alt]
notification          level, text
divider | spacer      [size<=512]
```

tone: neutral | muted | faint | accent | success | warning | error。

## 附录 D. Surface 1.1 字段

```text
文档      schema_version="1.1", bindings{}, hidden_actions[], root
绑定      capability, [operation], [input], [principal], [required], [mode], [debounce_ms]
slot      [id], surface, [version], [required], [props], [sizing], [fallback]
sizing    weight 0.1-20, [min], [max], [overflow scroll|clip|fit]
节点      id?（交互节点必填）, type, 其余同 1.0
```

## 附录 E. 能力速查

读：`workspace.read`、`extension.storage`、`events.subscribe`。
写：`workspace.write`、`events.publish`、`ui.panels`、`ui.commands`、`ui.theme`、`agent.tools.register`、`agent.context.provide`、`agent.prompts.provide`。
中：`agent.skills.provide`、`agent.models.provide`、`agent.agents.provide`、`agent.workflows.provide`。
高/严重：`network.http`、`hooks.observe`、`agent.intercept`、`process.execute`、`computer.use`、`secrets.read`。

## 附录 F. 钩子载荷示例

```json
{
  "point": "tool.before",
  "batch": [
    {
      "agent": "assistant",
      "data": {
        "tool_name": "hello__echo",
        "call_id": "call-1",
        "tool_args": "{\"text\":\"hi\"}"
      }
    }
  ]
}
```

http.exchange 不包 data：source、request_id、method、host、path、status、request_bytes、response_bytes、content_type、ttfb_ms、total_ms、error、request_headers、response_headers、request_body、response_body、body_truncated。

## 附录 G. 审查清单

- [ ] manifest 只有 2.0；id/版本/兼容范围合法；
- [ ] 每个声明的贡献都有文件，每个文件都被声明；
- [ ] 能力最小化；每个能力都能在代码或工具描述符里指出使用点；
- [ ] 工具 schema 有 required/additionalProperties；作用域参数可被策略提取；
- [ ] 运行时选择与隔离需求匹配；process/MCP 的 `process.execute` 是明确信任决定；
- [ ] 输入/输出有界；不把用户文档写进扩展存储，不把缓存写进工作区；
- [ ] 面板文档通过限额；交互节点有稳定 id；1.1 绑定可解析；
- [ ] 事件只订阅需要的主题；钩子只声明需要的点并导出入口；
- [ ] 秘密不进提示、日志、错误、面板；
- [ ] 失败路径有可读错误，且不破坏会话；
- [ ] 签名、哈希、锁、回滚路径可用。

## 附录 H. 术语

| 术语 | 含义 |
|---|---|
| 贡献 / contribution | `{kind,id,payload}`，包向宿主声明的一项能力 |
| 表面 / surface | 被贡献实际触达的宿主面（模型、读者、设置） |
| 限定名 | `namespace__contribution`，模型看到的工具名 |
| 能力 / capability | 授权词汇，不等于权限本身 |
| grant | 能力 + 作用域的一次授权 |
| 运行时 / runtime | 执行贡献的引擎（七种） |
| 激活 / activation | 运行时 + 贡献租约的一次生命 |
| 租约 / lease | 贡献的撤销凭据，manager 持有 |
| 审计 / audit | 每次能力决策的结构化记录 |
| 钩子 / hook | 同步 seam 的只读观测批次 |
| 槽位 / slot | 宿主或 surface 提供的挂载点 |
| surface 1.1 | 带绑定表、稳定 id、slot 的面板文档模式 |
| 联邦 / federation | 多 BSH 节点的信任、能力暴露与任务委派 |

## 变更记录（4.x）

- 面板 `view` 支持 schema 1.1：绑定表、稳定节点 id、slot、组合与动作路由；包可直接在 `panels/<id>.json` 使用。
- 宿主槽位注册表（11 个）：占用规则、Agent 准入、第一方专属在装载时强制。
- Agent surface 工具（present/patch/close/catalogue）：agent 限额 120 节点 / 16 KiB / 最多 3 个，Agent 徽标，动作只读。
- surface 消息总线（`surface.message`）：frame/close，action 从传输层拒绝，2 MiB 上限，远端绑定校验。
- cordis-rs surface provider runtime 移入 `ferris-runtime/src/surface_runtime.rs`，与 JS Cordis 适配器分名。
- 未知裸子命令直接报错退出 2（拼错不再静默启动 GUI），锁警告给出 `--federation-profile` 与 `federation status` 指引。
- 联邦发现：单 peer 5 秒超时、并发探测，死 peer 不再拖慢整轮。
