# Preset 授权分级规划

## 0. 实施状态

本规划的 Phase 1-4 已实现到当前代码：

- Preset 已支持 level、permission、read/write/deny、egress、web_search、mcp_tools、builtin_policy、policy_mode；
- 有效权限取会话与 Preset 的最小值，路径范围与拒绝路径进入策略数据；
- 内置 Rego 模块：deny-secrets、read-only、no-shell、no-egress、docs-only、deny-git-writes；
- 内置 Preset：standard、ptc、creator、read-only；
- 设置页按 Built-in/Custom 分组，卡片显示等级、权限、读写范围、写入/网络/策略状态，Details 展示完整生效配置；
- 未信任的 required policy 在发送门和 UI 的“Use in this chat”同时拒绝；
- 覆盖测试：权限 cap、路径范围、敏感路径、egress 交集、未信任策略、Use in this chat。

仍留待后续：

- 升权二次确认弹窗（当前用卡片上的“Writes files”/等级/范围提示）；
- MCP 连接级 egress（当前 egress 由每轮策略执行，共享 MCP pool 不重连）；
- bash 窄写范围的按范围 Seatbelt（当前窄范围关闭 shell）；
- admin Preset 与可视化 Preset 编辑器（当前用设置文件编辑，UI 展示与选择）。

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
    level = "docs"                    # inspect | review | research | docs | code | admin
    permission = "workspace-write"    # Preset 的权限上限
    read = ["."]                      # 工作区相对路径；"." = 整个工作区；[] = 不读
    write = ["docs", "README.md"]     # [] = 不写；None = 继承工作区
    deny = [".git", ".bsh", ".env", "secrets", ".ssh", ".aws"]
    egress = []                       # None = 继承工作区 allowlist；[] = 禁止；列表 = 交集
    web_search = false                # None = 继承模型设置
    plugins = ["files"]               # 核心 bundle：shell | files | plan
    tools = ["read_file", "write_file", "edit_file", "list_dir"]
    skills = ["docs"]
    mcps = []                         # server namespace 白名单
    mcp_tools = []                    # 可选：namespace_tool 级白名单
    extensions = []
    system_prompt = "You edit documentation only..."
    builtin_policy = ["deny-secrets", "docs-only", "no-shell", "no-egress"]
    policy_mode = "required"          # required | advisory
    trusted = true
    policy = ""                       # 可选自定义 Rego，required 时必须 trusted

字段语义：

| 字段 | 含义 |
|---|---|
| level | UI 分组和风险提示；不直接参与执行 |
| permission | Preset 权限上限；与会话权限取 min |
| read | 读白名单；None = 整个工作区，[] = 完全不读 |
| write | 写白名单；None = 整个工作区，[] = 完全不写 |
| deny | 拒绝前缀/精确路径；始终取并集，优先级最高 |
| egress | Preset 网络白名单；与会话 allowlist 取交集 |
| web_search | 是否允许 hosted web search；None 继承 |
| plugins | 核心 bundle 列表；空列表 = 不挂载 |
| tools | 内置工具白名单；None = 全部，[] = 无 |
| skills | 技能白名单；None = 全部启用，[] = 无 |
| mcps | MCP server namespace 白名单 |
| mcp_tools | 可选 MCP 工具级白名单，避免 server 级过宽 |
| extensions | 已安装扩展的 settings id 白名单 |
| system_prompt | LLM 人设；覆盖工作区 prompt |
| builtin_policy | 内置 Rego 模块 ID，二进制自带、默认可信 |
| policy_mode | required：未信任的自定义 policy 使 Preset 不可选；advisory：忽略并提示 |
| policy/trusted | 自定义 Rego 及其信任开关 |

路径规则：

- 只接受工作区相对路径，规范化后必须是工作区子路径；
- 拒绝 .. 逃逸、绝对路径、符号链接逃逸；
- deny 在 read/write 之后判定，始终优先；
- .git、.bsh 默认 deny，Preset 不能移除；
- 读/写范围为空表示没有权限，而不是“继承全部”。

## 5. 内置 Preset

### Standard

- 默认 Preset；挂载 shell、files、plan；
- 继承会话权限与工作区策略；deny secrets 和 Git internals；
- 人设：资深工程师，先读后改，运行针对性测试并报告实际命令和结果。

### PTC

- 与 Standard 相同的 bundle、权限和工作区范围；
- parallel_tool_calls = true；
- 人设：批量调用工具，然后过滤、整理、去重、计数、汇总；
- 这是 bsh 对 DSH PTC 的可用映射，不假装存在 host PTC runtime。

### Creator

- 与 Standard 相同的 bundle、权限和工作区范围；
- 人设：通过对话检查现有扩展/设置格式，编写扩展包、技能、预设；保持改动可回退；
- 这是 bsh 对 DSH Creator 的可用映射，不假装存在 Cordis 插件编排。

### Read-only

- permission: read-only；read: ["."]；write: []；
- deny 敏感路径；tools: read_file、list_dir、update_plan；
- 无 shell、无 MCP/扩展；web_search: true；
- builtin_policy: deny-secrets、read-only、no-shell、no-egress；
- 人设：只读分析师；可搜索，但不修改、不执行命令。

## 6. 完整示例

## 6. 完整示例

## 6. 完整示例

### read-only

    [presets.read-only]
    level = "read-only"
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
    builtin_policy = ["deny-secrets", "read-only", "no-shell", "no-egress"]
    policy_mode = "required"
    system_prompt = "You are a read-only analyst. Read and search, but never modify or run commands."

### ptc

    [presets.ptc]
    level = "ptc"
    plugins = ["shell", "files", "plan"]
    web_search = true
    parallel_tool_calls = true
    system_prompt = "Batch tool calls, then filter, organize, deduplicate, count and summarize."

### creator

    [presets.creator]
    level = "creator"
    plugins = ["shell", "files", "plan"]
    web_search = true
    system_prompt = "Extend bsh through conversation: write extension packages, skills and presets."

### 自定义 docs 写范围

内置目录不包含 Docs 类型；需要“只写文档”时，声明一个自定义 Preset：

    [presets.docs]
    level = "docs"
    permission = "workspace-write"
    read = ["."]
    write = ["docs", "README.md"]
    deny = [".git", ".bsh", ".env", "secrets"]
    plugins = ["files"]
    tools = ["read_file", "write_file", "edit_file", "list_dir"]
    web_search = false
    egress = []
    builtin_policy = ["deny-secrets", "docs-only", "no-shell", "no-egress"]
    policy_mode = "required"
    system_prompt = "You edit documentation only. Preserve code fences and frontmatter."

## 7. 执行映射

### 7.1 权限

- effective_permission = min(session.permission, preset.permission)
- read_only：不注册写工具、shell；文件工具只读
- workspace_write：文件工具 root = 工作区；bash 使用 Seatbelt
- full_access：仅会话显式设置；Preset 默认不提供

### 7.2 路径

在 Rego 数据中增加 Preset scope：

    data.preset.read_roots
    data.preset.write_roots
    data.preset.deny_roots

baseline 判定顺序：

1. 规范化路径；
2. deny 命中则拒绝；
3. read 动作要求命中 read_roots；
4. write 动作要求命中 write_roots；
5. 再与工作区策略、自定义策略取交集。

文件工具把绝对路径交给策略；策略用规范化前缀比较。bash 不走词法扫描作为安全边界。

### 7.3 bash

- L0/L1/L2：不注册 bash，或策略拒绝 spawn；
- L3：Seatbelt profile 由写范围生成，只允许 write_roots，拒绝 deny_roots；
- 若写范围窄于工作区，第一版可以**直接关闭 bash**，避免“策略允许但 shell 绕过”；
- 后续再实现按范围生成的 Seatbelt profile。网络在 shell 内目前不受 egress gate 约束，这是已知缺口；L0-L2 关闭 shell 即可规避。

### 7.4 工具、技能、MCP、扩展

- tools：Preset 白名单 ∩ 权限允许 ∩ bundle 实际提供；
- skills：Preset 白名单 ∩ 已启用；
- mcps：server namespace 白名单；mcp_tools 进一步限制到工具名；
- extensions：settings id 白名单 ∩ 已启用；
- 任何一项为 [] 都表示“没有”，不是“全部”。

### 7.5 egress 与 web_search

- egress None：继承工作区 allowlist；
- egress []：禁止所有出站；
- egress 列表：与工作区 allowlist 取交集；
- web_search false：不向模型提供 hosted web search；
- MCP HTTP 仍经过 egress gate；
- 域名与 IP：当前 gate 是 resolved-IP/CIDR，Preset 先按 CIDR 表达；域名白名单需要额外的解析策略，列为后续项。

### 7.6 策略

- 组合顺序：baseline + 工作区 .bsh/policy.rego + builtin_policy + 自定义 policy；
- builtin_policy 是二进制内置模块，默认可信，用户不能编辑；
- 自定义 policy 在 policy_mode = required 时必须 trusted，否则 Preset 不可选；
- 任何来源都只能收窄，不能放宽；
- 拒绝信息包含 Preset id、规则 id、中英文 refusal。

### 7.7 人设

- system_prompt 覆盖工作区 prompt；
- 人设只用于行为引导，不作为安全边界；
- 建议每个内置人设都包含一句“限制由外部策略执行，不要尝试绕过”；
- 人设进入 LLM 的 system/persona，已有测试覆盖。

## 8. UI 与会话选择

- Agent presets 采用 DSH 风格：Built-in 两列卡片网格，Custom 区块在下方；
- 卡片展示名称、Built-in/Default/In this chat 标签、等级、id、描述；页脚是 Mode details、信任、Use in this chat 和文档图标；
- 无 Preset 是 Built-in 上方的紧凑行，不占一张模式卡；
- Custom 卡片有 Edit 和 Delete：
  - Edit 在面板内打开 TOML 编辑器（[preset] 表），保存后写回设置草稿；
  - Delete 从草稿移除，并清除指向它的 global default 和当前会话引用；
  - 两者都不直接提交设置；Save settings 才持久化；
- Custom 区块底部是虚线 Create a custom preset；点击写入一个安全的只读模板并展开详情；
- Security 行列出实际生效的策略源（baseline、scope、egress、permission、builtin、custom），不再用“No policy”表示没有自定义模块；
- read-only 上限在未引用内置 read-only 模块时生成 preset-permission Rego，写操作由策略拒绝；
- “Use in this chat” 在以下情况禁用并说明原因：
  - 自定义 policy 未信任且 policy_mode = required；
- 会话 chip 显示名称；新会话默认 Preset 来自全局 default，会话可覆盖；
- 升权二次确认仍是后续项，目前用等级/权限/路径/Details 提示。

## 9. 实施阶段

### Phase 1：数据模型

- Preset 增加 level、permission、read、write、deny、egress、web_search、mcp_tools、builtin_policy、policy_mode；
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

## 11. 待定决策

1. bash 窄范围：第一版关闭 shell，还是直接做按 write_roots 生成 Seatbelt？
2. egress 是否支持域名白名单，还是只支持 CIDR？
3. MCP 是否需要工具级白名单（mcp_tools），还是 server 级足够？
4. 是否下发 admin Preset，还是只保留会话 Full access？
5. 内置 Preset 是否允许用户用同 id 覆盖；覆盖后是否仍标记为 built-in？
6. policy_mode = required 是否作为所有内置 Preset 的默认？

当前四个 Preset 已覆盖“读、写、命令、网络、策略”五类边界；后续可只增加真正有运行时支撑的类型。
