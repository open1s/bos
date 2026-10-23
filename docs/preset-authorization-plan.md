# 预设授权方案

> **状态说明（后续变更）**：本文件是历史规划稿。沙箱已经统一为 **heel**（唯一实现，无 fallback），
> 因此下文提到的 Seatbelt / SBPL 属于已被取代的设计；当前的策略、egress 与权限契约见
> `docs/bsh-tool-policy.md`，沙箱的平台行为见 `docs/terminal-platforms.md`。

## 沙箱开关（现行）

Preset 现在可以按预设选择是否应用 heel：新增字段 `sandbox = "heel" | "none"`（缺省 =
继承今天的行为：有效权限需要限制时使用 heel）。这是**每个 Preset 的策略选择**，不是重新引入
构建期 fallback，也没有恢复 Seatbelt/SBPL；heel 仍是唯一的沙箱实现和必需依赖。

- `heel`：强制受限，即使会话权限是完全访问。这是收窄方向。
- `none`：不受限运行，但**只有在 Preset 自身声明 `permission = "danger-full-access"`
  且会话也是完全访问时才允许**。任何更窄的上限（read-only / workspace-write / 未声明）都会
  以规则 `preset-sandbox-widens` 拒绝该选择并拒绝发送，Shell 保持受限；发送门与预设选择处
  都会显示该拒绝。若通过其他路径仍然构建了该 Preset，它的其他限制照常生效，沙箱解析结果仍是
  heel，绝不会出现“静默不受限”的 Shell。
- **开关只作用于 Shell 与终端，不作用于文件工具**：文件工具继续按权限层级和各自的 Rego
  检查执行；`none` 不会解除它们，`heel` 也不会给它们再加一层沙箱。
- 在已经是完全访问的会话里，`none` 只是把“今天的权限释放”显式写出来，本身不改变行为；
  真正改变行为的是 `heel`（在完全访问下强制收窄）。
- 未沙箱运行的含义必须说清：Shell 命令**同时失去文件系统限制和 egress 强制执行**。工作区
  `.bsh/egress-allowlist` 由 heel 的代理加逐连接 Rego 检查执行；没有 heel 就没有代理，
  命令可以访问白名单之外的地址。工具边界策略仍然生效：命令行明确写出的路径与端点仍会被
  Rego 检查并拒绝，运行时拼接出来的目标则不会被看到。
- 结果与审计必须诚实：bash 结果的 `sandbox` 为 `none` 时一定带 `sandbox_reason`：
  预设关闭时为 “a preset turned the sandbox off”，权限释放时为
  “the full-access permission released the sandbox”。预设关闭时，审计调用详情、终端状态行
  （含 `BSH_SANDBOX=none`）和转录中的命令说明显示同一状态；权限释放则在审计详情与原始结果中
  说明，因为那是完全访问的常态而不是策略选择。
- 非法值（例如 `sandbox = "seatbelt"`）是命名该字段的加载错误，不会静默回退。

## 0. 实施状态

本规划的 Phase 1-6 已实现到当前代码。下面每一条都能在代码里指出落点：

- Preset 支持 level、permission、sandbox、read/write/deny、egress、web_search、
  mcp_tools、builtin_policy、policy_mode，且全部 serde default，旧文件照常加载；
- 有效权限取会话与 Preset 的最小值（`PermissionResolution::resolve`），路径范围、
  路径 glob 与拒绝路径进入**同一个** `PolicySet`；
- 路径范围按 glob 语义执行（`crates/agent/src/policy/scope.rs`），deny 先判定；
- 内置 Rego 模块有稳定 id（`builtin:*`），默认可信，且用户模块不能冒用；
- 内置 Preset：standard、ptc、creator、read-only，并映射到授权等级 1/2/2/2；
- 设置页按 Built-in/Custom 分组，卡片显示等级徽章，Mode details 展示**解析后**的
  权限、读写拒绝范围、egress、web_search、policy_mode；
- 升权二次确认：等级升高或策略变为 advisory 时，任何选择入口（卡片、Use in this
  chat、TOML 编辑器）都会先停留在一个命名后果的确认框；取消保留原值；
- 未信任的 required policy 在发送门和 UI 的“Use in this chat”同时拒绝；
- 覆盖测试：权限 cap、字面/glob 路径范围、敏感路径、egress 交集、advisory/required、
  web_search 工具移除、内置 id 与冒用拒绝、等级表、风险确认的取消与确认。

仍留待后续（有运行时缺口，不是 UI 欠账）：

- MCP 连接级 egress（当前 egress 由每轮策略执行，共享 MCP pool 不重连）；
- bash 窄写范围的按范围内核限制（当前窄范围关闭 shell）；
- admin Preset 与可视化 Preset 编辑器（当前用设置文件/TOML 编辑器）。

## 0.1 授权等级与字段语义（现行）

### 等级表

`level = 0..=4`。缺省时由 [`Preset::derived_level`] 从**声明本身**推导，绝不默认成
一个与代码矛盾的数字；超出 0..=4 或未知名称是命名 `level` 字段的加载错误。

| level | 名称 | 随附 Preset | 实际授予（推导依据） |
|---|---|---|---|
| 0 | Inspect | 无 | 不挂载任何能行动的 bundle/tools/MCP/扩展，read 与 write 均为 `[]`，egress 为 `[]`，不允许 hosted search |
| 1 | Read-only | read-only | permission = read-only；可读工作区（read 声明存在），无写、无 shell、本机 egress = `[]`；hosted search 由 web_search 显式声明 |
| 2 | Standard | standard、ptc、creator | 未声明 permission，因此继承会话层级；保留 shell/files/plan，写入被工作区根约束，egress 继承工作区 |
| 3 | Elevated | 无 | permission = danger-full-access 且 `sandbox = "heel"`：权限上限放开但 shell 仍被限制 |
| 4 | Full access | 无 | permission = danger-full-access 且 sandbox 缺省/为 `none`：shell 随完全访问释放 |

推导规则（`Preset::derived_level`）：permission 为 full access 时看 sandbox（heel → 3，
否则 → 4）；不授予任何能力时 → 0；permission 为 read-only → 1；其余 → 2。
standard/ptc/creator 的字段完全相同（只有 persona 与 parallel_tool_calls 不同），
所以它们**必须**是同一等级；给 ptc 或 creator 单独发明等级会与代码矛盾。

### 路径范围 `read` / `write` / `deny`

- 一个条目可以是**字面路径**（`docs`、`README.md`、`/abs/logs`）或**glob**
  （含 `*`、`?`、`[`、`{`，例如 `src/**/*.rs`）。字面条目匹配自身与整棵子树，
  与历史行为一致；glob 中 `*` 不跨 `/`，`**` 跨目录。
- 相对条目相对于工作区根；绝对条目允许。读/写仍被 baseline 限制在工作区内，
  所以绝对读写条目不会扩大可达范围；绝对 deny 条目有效。
- **交集，永不并集**：权限层级、工作区根、Preset 范围同时约束；Preset 声明了
  read/write 时，不在列表中的路径被拒绝（`preset-read-scope` / `preset-write-scope`），
  而不是“继承全部”。
- **deny 先判定且最高优先**：在 read/write 之前（`preset-deny-path`），即使层级允许、
  即使路径在工作区内也拒绝；deny 与 baseline 的 `.git`/`.bsh` 并集。
- 字段语义：`read` 缺省 = 整个工作区，`[]` = 不读；`write` 缺省 = 整个工作区，
  `[]` = 不写；`deny` 缺省 = 空，始终并入 baseline 的拒绝项。
- **加载错误，不是静默默认**：空条目、`~`、含 `..` 的条目、无法解析的 glob
  （未闭合的 `[`/`{`、不是完整路径组件的 `**` 等）都会被 `GlobalSettings::validate`
  以 `SettingsError::InvalidPreset` 拒绝，消息包含 preset id、字段名（read/write/deny）
  与具体原因。TOML 编辑器的保存路径调用同一个 `validate_preset`。

### `web_search`

`true` 显式授予；`false` **从请求的工具数组中移除** hosted web-search 工具
（`AgentConfig::web_search`，由 `offers_web_search(enabled, api_mode)` 只在
Responses 协议下提供）；缺省继承工作区/模型的 `web_tools_enabled`。这与
`egress = []` 拒绝网络是同一种“缺席而不是调用时拒绝”的做法：工具根本不在请求里。
hosted search 跑在模型供应商的服务器上，不是本机 egress，两者互不替代。

### `policy_mode`

- `required`（缺省）：策略拒绝即硬拒绝，工具返回带规则 id 与原因的 refusal。
- `advisory`：**Preset 自己贡献的模块**（preset scope、preset egress、生成/命名的
  内置 guard、Preset 自定义 Rego）的拒绝会被**记录**在审计环里（`advisory: true`），
  调用继续执行，并在工具返回的 decision reason、策略面板的“Advisory refusals”与
  状态栏通知（`Policy advisory: {rule} was refused and the call proceeded.`）里可见
  （`advisory: Refused by policy: ...`）。状态栏通知只在 advisory 计数增长时设置一次，
  不随每帧重复。
- advisory **不会**放宽 baseline、会话层级与工作区策略：这些模块从不被标记为
  advisory，所以工作区外的读写、层级禁止的写/egress/spawn 仍然硬拒绝。
  `PolicySet::probe`（沙箱规格推导）也永远使用严格答案，因此 shell 的受限范围
  不会因为 advisory 而扩大。
- 判定点在 `PolicySet::check` 一处：引擎的 Rego 判定不变，只有 host 决定是否
  让告警替代拒绝。

### 内置 Rego 模块 id 与信任

| id | 来源 | 何时编译 | 信任 |
|---|---|---|---|
| `builtin:baseline` | 二进制 | 总是 | 默认可信 |
| `builtin:session-permission` | 二进制（按有效层级生成） | 总是 | 默认可信 |
| `builtin:preset-scope` | 二进制 | Preset 声明 read/write/deny 时 | 默认可信 |
| `builtin:preset-egress` | 二进制 | Preset 声明 egress 时 | 默认可信 |
| `builtin:preset-permission` | 二进制（生成） | read-only 上限且未命名 `builtin:read-only` 时 | 默认可信 |
| `builtin:deny-secrets` | 二进制 | Preset 的 `builtin_policy` 命名时 | 默认可信 |
| `builtin:read-only` | 二进制 | 同上 | 默认可信 |
| `builtin:no-shell` | 二进制 | 同上 | 默认可信 |
| `builtin:no-egress` | 二进制 | 同上 | 默认可信 |
| `builtin:docs-only` | 二进制 | 同上 | 默认可信 |
| `builtin:deny-git-writes` | 二进制 | 同上 | 默认可信 |

- 旧写法 `builtin_policy = ["deny-secrets"]` 仍然接受，加载时规范化为
  `builtin:deny-secrets`；面板与卡片显示规范 id。
- **保留命名空间**：`builtin:` 前缀属于二进制。非 built-in 来源（工作区
  `.bsh/policy.rego` 或扩展/Preset 自定义 Rego）如果通过模块头部
  `# module: builtin:xxx` 声明内置 id，`PolicySet::compile` 直接失败
  （`PolicyError::BuiltinIdClaimed`），消息同时指出声明来源与它试图冒用的内置 id。
- 来源与信任门：
  - `builtin:*`：二进制自带，默认可信；
  - `extension`（Preset/扩展贡献的 Rego）：用 Preset 的 `trusted` 门，未信任且
    required 的 Preset 不可用、模块不进入编译；advisory 下未信任的自定义 Rego 被忽略；
  - `workspace`（读者自己的 `.bsh/policy.rego`）：信任来自**位置**而不是一个额外的
    开关——baseline 拒绝一切写 `.bsh` 的工具调用，所以只有读者本人能在那里放文件；
    面板把它列为 trusted (workspace file)。不新增“未信任的 workspace 模块”状态，
    因为那会让升级后已有的工作区策略在读者重新信任前静默失效。
  面板为每个模块列出 id、来源（built-in / workspace / extension）与信任状态。

## 1. 目标

让每个会话选择一个 Preset，用一套声明式配置同时约束：

- 文件/目录的读、写、拒绝范围；
- 安全策略（Rego、egress、shell 沙箱、权限上限）；
- TOOL、SKILL、PLUGIN/扩展、MCP；
- LLM 人设与系统提示；
- 内置的 Rego 安全模块。

核心原则：**Preset 只能收窄，不能放宽**。工作区策略、用户权限和 Preset 取交集；拒绝范围取并集；Preset 不能绕开工作区已有的限制。

## 2. 现状与缺口

已经具备：

- Preset 的 plugins、tools、skills、mcps、extensions、system_prompt、policy、trusted；
- 会话级选择（hero/header chip、设置页 Use in this chat）；
- 构建时应用上述配置，persona 进入 LLM 的 system/persona；
- 工具边界 Rego 策略、bash Seatbelt、egress gate、审计。

本规划要补齐：

- Preset 的 permission 上限；
- read/write/deny 路径范围；
- egress 与 web_search 控制；
- 内置 Rego 模块 ID（binary-shipped，默认可信）；
- policy_mode（required/advisory）；
- 授权等级 level 与内置 Preset 目录；
- 设置页的分级卡片、范围展示和风险确认。

## 3. 四种 Preset

参考 DSH 的四种内置模式，并保留一个只读模式。Minimal 不再单独提供；纯对话可以用自定义空 Preset，或直接选择 Read-only（只读、可搜索）。

| Preset | permission | 写范围 | shell | 网络 | 用途 |
|---|---|---|---|---|---|
| Standard | 继承会话 | 工作区 | 沙箱 | 继承 | 日常编码、默认 |
| PTC | 继承会话 | 工作区 | 沙箱 | 继承 | 批量工具调用后整理/去重/汇总 |
| Creator | 继承会话 | 工作区 | 沙箱 | 继承 | 通过对话编写扩展/技能/预设 |
| Read-only | read-only | 无 | 禁止 | 仅 hosted web search | 阅读、审查、研究 |

PTC 通过 parallel_tool_calls 和批量工具人设实现；Creator 通过扩展编写人设和标准工作区写入实现。二者不引入新的宿主运行时。

## 4. Preset 配置模型

建议新增字段（全部 serde default，旧 Preset 保持兼容）：

    [presets.example]
    level = 2                         # 0..=4；缺省从实际授予推导（见 0.1 的等级表）
    permission = "workspace-write"    # Preset 的权限上限
    sandbox = "heel"                  # heel | none；None = 继承权限行为
    read = ["."]                      # 相对/绝对路径或 glob；"." = 整个工作区；[] = 不读
    write = ["docs", "README.md", "src/**/*.rs"]  # [] = 不写；None = 继承工作区
    deny = [".git", ".bsh", ".env", "secrets", ".ssh", ".aws", "**/*.pem"]
    egress = []                       # None = 继承工作区（文件不存在时即不受限）；[] = 禁止；列表 = 交集
    web_search = false                # false = 从请求中移除 hosted web-search 工具
    plugins = ["files"]               # 核心 bundle：shell | files | plan
    tools = ["read_file", "write_file", "edit_file", "list_dir"]
    skills = ["docs"]
    mcps = []                         # server namespace 白名单
    mcp_tools = []                    # 可选：namespace_tool 级白名单
    extensions = []
    system_prompt = "You edit documentation only..."
    builtin_policy = ["builtin:deny-secrets", "builtin:docs-only", "builtin:no-shell", "builtin:no-egress"]
    policy_mode = "required"          # required | advisory
    trusted = true
    policy = ""                       # 可选自定义 Rego，required 时必须 trusted

字段语义：

| 字段 | 含义 |
|---|---|
| level | 授权等级 0..=4（见 0.1）；缺省时从 permission/sandbox/实际授予推导；非法值是加载错误 |
| permission | Preset 权限上限；与会话权限取 min |
| sandbox | Shell 沙箱：heel = 强制受限（可在完全访问下收窄）；none = 不受限，仅当 Preset 与会话都是完全访问时才允许，否则以 preset-sandbox-widens 拒绝；None = 继承 |
| read | 读白名单：相对/绝对路径或 glob；None = 整个工作区，[] = 完全不读；声明后不在列表内的读取被拒绝 |
| write | 写白名单：同上；None = 整个工作区，[] = 完全不写；仍受工作区根与权限层级约束 |
| deny | 拒绝路径/glob；在 read/write 之前判定，优先级最高，始终并入 baseline 的 .git/.bsh |
| egress | Preset 网络白名单；与会话 allowlist 取交集。None = 继承：工作区文件不存在时是“不受限”，存在但为空时是“全部拒绝” |
| web_search | false 直接把 hosted web-search 工具从请求中移除；None 继承 web_tools_enabled |
| plugins | 核心 bundle 列表；空列表 = 不挂载 |
| tools | 内置工具白名单；None = 全部，[] = 无 |
| skills | 技能白名单；None = 全部启用，[] = 无 |
| mcps | MCP server namespace 白名单 |
| mcp_tools | 可选 MCP 工具级白名单，避免 server 级过宽 |
| extensions | 已安装扩展的 settings id 白名单 |
| system_prompt | LLM 人设；覆盖工作区 prompt |
| builtin_policy | 内置 Rego 模块 ID（builtin:*，二进制自带、默认可信）；旧的无前缀名作为别名接受 |
| policy_mode | required：拒绝即停止；advisory：Preset 自己模块的拒绝被记录、调用继续（baseline/层级/工作区仍硬拒绝） |
| policy/trusted | 自定义 Rego 及其信任开关；required + 未信任使 Preset 不可用 |

路径规则：

- 条目是工作区相对路径或绝对路径，也可以含 glob（`*`、`?`、`[`、`{`）；
- 相对条目规范化后必须仍在工作区内；`..` 逃逸是加载错误，不是静默丢弃；
- 绝对读写条目允许，但 baseline 仍把读写限制在工作区内；绝对 deny 有效；
- deny 在 read/write 之前判定，始终优先，即使层级允许、即使在工作区内；
- .git、.bsh 默认 deny，Preset 不能移除；
- 读/写范围为空表示没有权限，而不是“继承全部”。

## 5. 内置 Preset

### Standard

- level 2（Standard）；默认 Preset；挂载 shell、files、plan；
- 继承会话权限与工作区策略；deny secrets 和 Git internals；
- 人设：资深工程师，先读后改，运行针对性测试并报告实际命令和结果。

### PTC

- level 2：与 Standard 相同的 bundle、权限和工作区范围（按“实际授予”测量，PTC 与
  Standard 同等级；只有 parallel_tool_calls 与人设不同）；
- parallel_tool_calls = true；
- 人设：批量调用工具，然后过滤、整理、去重、计数、汇总；
- 这是 bsh 对 DSH PTC 的可用映射，不假装存在 host PTC runtime。

### Creator

- level 2：与 Standard 相同的 bundle、权限和工作区范围（人设不同，授予相同）；
- 人设：通过对话检查现有扩展/设置格式，编写扩展包、技能、预设；保持改动可回退；
- 这是 bsh 对 DSH Creator 的可用映射，不假装存在 Cordis 插件编排。

### Read-only

- level 1（Read-only）：permission = read-only；read: ["."]；write: []；
- deny 敏感路径；tools: read_file、list_dir、update_plan；
- 无 shell、无 MCP/扩展；web_search: true；
- builtin_policy: builtin:deny-secrets、builtin:read-only、builtin:no-shell、builtin:no-egress；
- 人设：只读分析师；可搜索，但不修改、不执行命令。

## 6. 完整示例

### read-only

    [presets.read-only]
    level = 1
    permission = "read-only"
    read = ["."]
    write = []
    deny = [".git", ".bsh", ".env", "secrets", ".ssh", ".aws"]
    plugins = ["files", "plan"]
    tools = ["read_file", "list_dir", "update_plan"]
    skills = []
    mcps = []
    extensions = []
    web_search = true
    egress = []
    builtin_policy = ["builtin:deny-secrets", "builtin:read-only", "builtin:no-shell", "builtin:no-egress"]
    policy_mode = "required"
    system_prompt = "You are a read-only analyst. Read and search, but never modify or run commands."

### ptc

    [presets.ptc]
    level = 2
    plugins = ["shell", "files", "plan"]
    web_search = true
    parallel_tool_calls = true
    system_prompt = "Batch tool calls, then filter, organize, deduplicate, count and summarize."

### creator

    [presets.creator]
    level = 2
    plugins = ["shell", "files", "plan"]
    web_search = true
    system_prompt = "Extend bsh through conversation: write extension packages, skills and presets."

### 自定义 docs 写范围

内置目录不包含 Docs 类型；需要“只写文档”时，声明一个自定义 Preset：

    [presets.docs]
    level = 2                         # workspace-write 上限 + 受限 shell
    permission = "workspace-write"
    read = ["."]
    write = ["docs", "README.md", "docs/**/*.md"]
    deny = [".git", ".bsh", ".env", "secrets"]
    plugins = ["files"]
    tools = ["read_file", "write_file", "edit_file", "list_dir"]
    web_search = false
    egress = []
    builtin_policy = ["builtin:deny-secrets", "builtin:docs-only", "builtin:no-shell", "builtin:no-egress"]
    policy_mode = "required"
    system_prompt = "You edit documentation only. Preserve code fences and frontmatter."

## 7. 执行映射

### 7.1 权限

- effective_permission = min(session.permission, preset.permission)
- read_only：不注册写工具、shell；文件工具只读
- workspace_write：文件工具 root = 工作区；bash 使用 Seatbelt
- full_access：仅会话显式设置；Preset 默认不提供

### 7.2 路径

在 Rego 数据中增加 Preset scope（字面前缀与 glob 平行，缺一不可）：

    data.preset_scope.read_roots    data.preset_scope.read_globs
    data.preset_scope.write_roots   data.preset_scope.write_globs
    data.preset_scope.deny_roots    data.preset_scope.deny_globs

判定顺序（`crates/agent/src/policy/scope.rs` 的 `policy.preset_scope` 模块）：

1. 文件工具把绝对化后的路径交给策略；
2. deny：命中字面 root（前缀）或 glob（`glob.match(pattern, ["/"], path)`）即拒绝；
3. read：声明了 read 范围而路径不在其中则拒绝；
4. write：声明了 write 范围而路径不在其中则拒绝；
5. 最后与 baseline、会话层级、工作区策略取交集（每个模块都必须 allow）。

字面条目编译为绝对前缀（`docs` 覆盖自身与整棵子树），glob 条目编译为绝对 pattern
（`src/**/*.rs`）；`.` 解析为工作区根本身。glob 仍走同一个 bytecode 引擎
（`glob.match` 在 regorus 中可被 RVM 编译），所以路径检查不会把整套策略降级为解释器。
bash 不走词法扫描作为安全边界。

### 7.3 bash

- L0/L1/L2：不注册 bash，或策略拒绝 spawn；
- L3：Seatbelt profile 由写范围生成，只允许 write_roots，拒绝 deny_roots；
- 若写范围窄于工作区，第一版可以**直接关闭 bash**，避免“策略允许但 shell 绕过”；
- 后续再实现按范围生成的 profile。网络在 shell 内由 egress gate 的派生规格约束（见 7.5）；L0-L2 关闭 shell 即可规避。

现行实现（heel 统一后）：`sandbox = "heel"` 在完全访问下也强制受限，属于收窄；`sandbox = "none"` 仅在 Preset 与会话都是完全访问时允许，否则以 `preset-sandbox-widens` 拒绝并保持受限。未沙箱时 Shell 同时失去文件系统的内核限制与 egress 代理执行（见 7.5），工具边界策略对命令行可见目标仍然生效。

### 7.4 工具、技能、MCP、扩展

- tools：Preset 白名单 ∩ 权限允许 ∩ bundle 实际提供；
- skills：Preset 白名单 ∩ 已启用；
- mcps：server namespace 白名单；mcp_tools 进一步限制到工具名；
- extensions：settings id 白名单 ∩ 已启用；
- 任何一项为 [] 都表示“没有”，不是“全部”。

### 7.5 egress 与 web_search

- 工作区 `.bsh/egress-allowlist` 是三态：**文件不存在 = 不受限**（新工作区默认可以联网，Standard/PTC 继承的就是这个状态）；**文件存在但无规则 = 全部拒绝**（读者创建文件即选择开启控制）；**文件存在且有规则 = 白名单**；
- egress None：继承工作区 allowlist，因此工作区文件不存在时是“不受限”；
- egress []：禁止所有出站，工作区不受限时也一样；
- egress 列表：与工作区 allowlist 取交集，永不取并集；工作区不受限时，Preset 列表就是全部可达范围；
- 注册/打开工作区不再自动写入起始文件（自动写入会把新工作区从“不受限”翻成“全部拒绝”）；只有 Policy 面板的显式操作为空文件写入起始内容，且该内容会拒绝所有出站；
- web_search false：**从请求的工具数组中移除** hosted web search（`config.web_search = false`，
  `offers_web_search` 因此不 push 该工具），不是调用时再拒绝；`web_search` 缺省继承
  工作区的 `web_tools_enabled`；
- MCP HTTP 仍经过 egress gate；
- **sandbox = "none" 时 egress 不被执行**：工作区 `.bsh/egress-allowlist` 由 heel 的代理加逐连接 Rego 检查执行，没有 heel 就没有代理；命令可以访问白名单之外的地址。命令行点名的端点仍会被命令层拒绝，运行时拼接的主机则不会被看到。这是关闭沙箱的既定代价，测试同时验证“none 可通、heel 被拒”；
- 域名与 IP：当前 gate 是 resolved-IP/CIDR，Preset 先按 CIDR 表达；域名白名单需要额外的解析策略，列为后续项。

### 7.6 策略

- 组合顺序：`builtin:baseline` + `builtin:session-permission` + Preset 的范围/egress/
  permission 模块 + 命名的 `builtin_policy` + Preset 自定义 policy + 工作区
  `.bsh/policy.rego`；每个模块都必须 allow（交集）；
- 每个模块有稳定 id 与来源：`builtin:*`（二进制，默认可信）、`workspace`（读者自己的
  `.bsh/policy.rego`）、`extension`（Preset/扩展贡献，经 `trusted` 门）；
- `builtin:` 是保留命名空间；用户模块通过 `# module: builtin:xxx` 冒用内置 id 时，
  `PolicySet::compile` 返回 `BuiltinIdClaimed`，消息同时指出来源与内置 id；
- 自定义 policy 在 policy_mode = required 时必须 trusted，否则 Preset 不可选；
- policy_mode = advisory 只让 **Preset 自己贡献的模块** 的拒绝变成“记录 + 继续”；
  baseline、会话层级、工作区策略永远硬拒绝，`probe`（沙箱规格）也永远用严格答案；
- 任何来源都只能收窄，不能放宽；
- 拒绝信息包含 Preset id、模块 id、规则 id、中英文 refusal；advisory 的 decision reason
  以 `advisory: ` 前缀携带同一句 refusal，审计条目带 `advisory: true`。

### 7.7 人设

- system_prompt 覆盖工作区 prompt；
- 人设只用于行为引导，不作为安全边界；
- 建议每个内置人设都包含一句“限制由外部策略执行，不要尝试绕过”；
- 人设进入 LLM 的 system/persona，已有测试覆盖。

## 8. UI 与会话选择

- Agent presets 采用 DSH 风格：Built-in 两列卡片网格，Custom 区块在下方；
- 卡片展示名称、Built-in/Default/In this chat 标签、**等级徽章**（`L{level} · 名称`，
  等级名称来自 0.1 的表，颜色随等级升高从 muted → success → warn）、id、描述；
  标题行同时显示紧凑的沙箱状态（Sandbox: heel / off / inherit / refused，不增加卡片高度），
  显式 off 或拒绝时正文再给出完整说明，拒绝状态以错误色呈现；
- Mode details 展示的是**解析后**的范围：
  - Access：会话权限与 Preset 上限取 min 后的有效层级；上限压低会话时追加“ceiling: …”；
  - Level：卡片同一等级；
  - Paths：resolved read/write/deny。read/write 的“整个工作区 / 无 / 条目列表”来自
    `ResolvedPresetScope`，read-only 会话即使 Preset 声明了写范围也显示“无”；
  - Network：声明的 egress 措辞 + “effective:” 后接与工作区 allowlist 取交集后的实际
    可达范围（工作区无文件 = 不受限；空文件 = 全部拒绝；有规则 = 交集）；
  - Web search：解析后的开/关（Preset 缺省继承工作区设置）；
  - Policy mode：required / advisory；
- 设置页的卡片从 `preset_catalogue::catalogue` 读取；该目录遍历 `preset_ids()` 并调用
  `preset(id)`，不是内置 Preset 的第二份副本，因此声明覆盖与构建结果永远一致；
- 页脚是 Mode details、Advanced（仅在 Mode details 展开时出现）、信任、Use in this chat
  和文档图标；“Advanced”在页脚而不是展开块内部：展开块从页脚下方开始，在参考窗口高度下
  会落到视口之外，放在块顶部仍会被裁剪（harness 点击是 silent no-op）。页脚是唯一保证
  可见的行；“Use in this chat”在沙箱拒绝时禁用并显示 `preset-sandbox-widens`，
  点击卡片设为默认也会被状态层拒绝并提示；
- 无 Preset 是 Built-in 上方的紧凑行，不占一张模式卡；
- Custom 卡片有 Edit 和 Delete：
  - Edit 在面板内打开 TOML 编辑器（[preset] 表），保存后写回设置草稿；
  - Delete 从草稿移除，并清除指向它的 global default 和当前会话引用；
  - 两者都不直接提交设置；Save settings 才持久化；
- Custom 区块底部是虚线 Create a custom preset；点击写入一个安全的只读模板并展开详情；
- Security 行列出实际生效的策略源，使用与 Policy 面板相同的稳定 id
  （`builtin:baseline`、`builtin:preset-scope`、`builtin:preset-egress`、
  `builtin:preset-permission`、命名的 `builtin:*`、custom），不再用“No policy”
  表示没有自定义模块；
- Policy 面板新增 “Policy modules” 区：每个模块一行 id · 来源（built-in / workspace /
  extension）· 信任状态，并列出它声明的 rule id；另有 “Advisory refusals” 区显示
  advisory 模式记录下来的拒绝（rule/tool/target/reason），审计列表给这些行加
  “advisory” 标记并以警示色显示；
- read-only 上限在未引用内置 read-only 模块时生成 preset-permission Rego，写操作由策略拒绝；
- “Use in this chat” 在以下情况禁用并说明原因：
  - 自定义 policy 未信任且 policy_mode = required；
- 会话 chip 显示名称；新会话默认 Preset 来自全局 default，会话可覆盖；
- **升权二次确认**（已实现）：当目标 Preset 的等级高于当前 Preset，或它的自定义策略从
  required 变成 advisory 时，选择不会立即写入，而是停在 state 里的 pending change；
  弹窗用**一句**说明把后果一次讲清（`widening_sentence`）：`danger-full-access`、
  `The shell runs unconfined: a preset turned the sandbox off.`（与 bash 结果同一句）、
  `Every outbound connection is refused (egress = []).`、`The preset's policy is
  advisory: a refusal is recorded and the call proceeds.` 逐条列出，以分号连接，
  不出现等级编号——读者不必先读等级表才能理解自己确认了什么。Cancel 只清掉 pending，
  原值不动；Confirm 才写入。三个入口共用同一道门：卡片正文/“Use in this chat”走
  `select_preset` /
  `select_session_preset`，TOML 编辑器保存走 `commit_preset`；
- 只收窄的选择（read-only、较低等级、同一声明）不需要确认，仍是单击即生效。
- **切换立即重编策略（live rebake）**：Preset、权限层级、Save 之后的设置都会调用
  `AppState::rebake_policy`，用已提交的 settings + 当前会话重新编译 `PolicySet`，
  所以终端 pane 的 sandbox 规格、审计/策略面板、`policy_generation` 驱动的能力列表
  都在同一帧看到新值，不必等到下一次回合。正在流式输出的回合例外：它的 agent、
  sandbox spec 与工具集是在 spawn 时烘死的，重编只能影响“读 live policy”的表面；
  此时选择处会显示 `Preset applied to the next turn; the running turn keeps its
  current build.` / `Applied to the next turn; the running turn keeps its current
  permission.`，下一次回合（包括排队消息 drain 出来的回合）用新配置重新构建。

## 9. 实施阶段

### Phase 1：数据模型

- Preset 增加 level、permission、sandbox、read、write、deny、egress、web_search、mcp_tools、builtin_policy、policy_mode；
- Resolution/Effective 携带这些字段；
- 旧 Preset serde default 兼容。

### Phase 2：有效值计算

- effective_permission = min；
- read/write/deny 规范化与交集；
- egress/tools/skills/mcps/extensions 交集；
- 单元测试覆盖“Preset 不能放宽”。

### Phase 3：策略执行

- baseline 增加 read_roots/write_roots/deny_roots；
- 内置 Rego 模块注册表；
- 文件工具、bash、egress 接入；
- 审计和 refusal 带上 Preset/规则。

### Phase 4：内置目录与 UI

- standard、ptc、creator、read-only 四个内置 Preset；
- DSH 风格卡片、Custom 区块、TOML 编辑器、删除、禁用原因；
- 设置页/会话 chip 显示生效值。

### Phase 5：测试与文档

- 授权矩阵、路径逃逸、symlink、权限 cap、未信任 policy、egress、MCP 工具级、UI；
- 文档：docs/bsh-tool-policy.md 增加 Preset 授权章节。

### Phase 6：本轮完成的部分（全部已落地）

- 等级改为 `Option<u8>`（0..=4）+ 推导 + 文档表；非法值加载失败；
- 路径范围支持 glob 与绝对条目；无法解析是命名 preset/字段/值的加载错误；
- 新策略数据字段 `*_globs`；`policy.preset_scope` 先 deny 后 read/write；
- web_search=false 从请求移除 hosted tool（engine 测试断言工具数组）；
- 内置模块稳定 id（`builtin:*`）、来源/信任元数据、冒用内置 id 的编译错误；
- policy_mode=advisory 在 `PolicySet::check` 一处生效，审计 + 面板可见；
- 设置页等级徽章、解析后范围、风险确认弹窗；en+zh 文案；
- 测试：`policy::tests_scope`、`preset_catalogue::tests`、caps 的 glob/advisory/
  web_search/absent-field 测试、shell 的风险门与模块列表测试。

## 10. 测试矩阵

| 场景 | read-only | ptc | standard |
|---|---|---|---|
| 读 src/main.rs | allow | allow | allow |
| 读 .env | deny | deny | deny |
| 写 docs/a.md | deny | allow | allow |
| 写 src/main.rs | deny | allow | allow |
| 写 .git/config | deny | deny | deny |
| 执行 bash | deny | allow（沙箱） | allow（沙箱） |
| 访问网络 | deny | 继承 | 继承 |
| 读工作区外 /etc/passwd | deny | deny | deny |
| .. 逃逸 | deny | deny | deny |
| symlink 逃逸 | deny | deny | deny |
| 会话 Full access + read-only | read-only | - | - |
| 未信任 required policy | 不可选 | 不可选 | 不可选 |
| deny 命中工作区内、层级允许的写 | deny | deny | deny |
| 写 glob 外（工作区内） | deny | deny | deny |
| read glob 内 / 外 | allow / deny | allow / deny | allow / deny |
| web_search = false 的请求工具数组 | 无 web_search | 无 web_search | 无 web_search |
| policy_mode = advisory 的拒绝 | 记录 + 继续 | 记录 + 继续 | 记录 + 继续 |
| advisory 下的工作区外写 | deny（baseline） | deny | deny |
| 用户模块声明 builtin:baseline | 加载错误 | 加载错误 | 加载错误 |
| 升权（2 → 4）选择 | 需确认，取消保留原值 | 同左 | 同左 |

## 11. 已决与仍开放

已决：

1. bash 窄范围仍关闭 shell：按写范围生成内核 profile 需要 per-scope profile，留待后续；
2. egress 支持地址/CIDR、主机名（含 `*.`）与端口三种形状；域名白名单不做额外 DNS 策略；
3. MCP 有工具级白名单（`mcp_tools`）；
4. 不下发 admin Preset：等级 4 只能由声明 full-access 的 Preset 表达，且要确认；
5. 内置 Preset 允许用户用同 id 覆盖（声明替换整份定义）；`preset_is_shipped` 随之为
   false，卡片不再显示 Built-in；
6. `required` 是所有 Preset 的默认 policy_mode（缺省即 required）。

仍开放：

- MCP 连接级 egress（当前按轮执行，共享 pool 不重连）；
- bash 窄写范围的按范围内核限制；
- 内置 Preset 的等级是否要拆出 3（full-access 上限 + heel）的随附预设：当前没有这样的
  内置预设，等级 3 只对自定义声明可达。

当前四个 Preset 已覆盖“读、写、命令、网络、策略”五类边界；后续可只增加真正有运行时支撑的类型。
