# BSH Intelligent Review Gap Analysis

- **Status:** Proposed
- **Version:** 0.1.0
- **Date:** 2026-11-09
- **Baseline:** BSH working copy slkkupuqztnx
- **Companion RFC:** [BSH Intelligent Review 架构 RFC](bsh-intelligent-review-rfc.md)
- **Method:** 对当前工作副本做静态代码审查；没有把任何“计划中的能力”当成已实现。本文不是 release audit，文件行号以 baseline 为准。
- **Implementation update:** 2026-11-09 的 MVP 已关闭 P0 的领域/存储/精确文件捕获，以及 P1 的 Review tab、结构 diff、digest-bound decision、Task/Audit 链接与跨 changeset 的 Audit decision timeline。下面的 Gap Register 保留为**实现前基线**；仍未关闭的项在 RFC 第 0 节列出。

---

## 1. 结论

BSH 的 Diff、Audit、Tasks 都是真实实现，不是空壳：

- Diff pane 能解析 patch、conflict、普通文件，并用 kit 的 Diff 渲染。
- Audit 是确定性的 tool-call/refusal 投影，不调用模型。
- Tasks 有持久化记录、attempt、run、scheduler、board。

但 **Intelligent Review 作为一个产品能力还不存在**。当前系统能回答“模型请求了什么”，不能回答“磁盘上真实变了什么”；能回答“哪些调用被拒绝”，不能回答“谁在哪个内容摘要上批准了什么”；能列出 task，不能把 task 与一次真实变更、一次 review、一次决定连起来。

能力成熟度：

| 能力 | 状态 | 结论 |
|---|---|---|
| Structural Review | 🔴 未实现 | 只有行级文本 patch，没有符号/结构树 |
| Multi-format Review | 🔴 未实现 | 非 UTF-8 直接拒绝；image/PDF/docx/archive 无 adapter |
| Agent-aware Review | 🟡 部分数据 | 有 tool/session/task 线索，但没有稳定 change identity，无法可靠联接 |
| Intelligent Verification | 🔴 未实现 | 只有 policy audit；没有 finding/evidence/verification/decision |
| Auditable Changes | 🟡 部分实现 | 有 tool-call audit，没有 content-addressed change history |
| GPUI Review surface | 🔴 未实现 | 主 tab 没有 Review；右栏 Diff 仍是低层比较器 |
| Task/Review/Audit 打通 | 🔴 未实现 | 三套模型没有 ID 级联接 |

**判定：在 P0 完成前，不应在 UI 或对外材料中宣称 Structural Review、Multi-format Review、Intelligent Verification 已支持。**

---

## 2. 现状盘点

### 2.1 已有能力

| 能力 | 实现 | 价值 |
|---|---|---|
| 文本 patch / conflict 比较 | crates/bsh/src/ui/diff_pane.rs；kit Diff | 可直接复用为 Review 的 raw diff 层 |
| 单轮写入文件列表 | turn_changes / turn_files | 已有 UI 入口，但数据形状是 tool request |
| 工具调用审计 | AuditView::build | 确定性、可测试、无模型调用 |
| Policy 拒绝审计 | denial ring + AuditPolicy | 能解释“为什么被拒绝” |
| Task 持久化 | FileTasksStore | 已有稳定 task id / attempt id |
| Task run seam | RunsService | 可观察/取消，适合挂 change set |
| 原子写 / per-path lock | write_bytes / file_lock | Snapshot 可复用的并发基础 |
| SHA-256 | extension package hashing | 不需要新增哈希依赖 |
| GPUI 测试基建 | debug_selector / click tests | Review 的交互可验证 |

### 2.2 关键代码证据

| 证据 | 位置 | 影响 |
|---|---|---|
| ToolEvent 无 call id / change id | crates/bsh/src/model.rs:21-40 | 不能稳定联接 tool/review/audit |
| write_file 不返回 before | crates/bsh/src/caps.rs:4964-5011 | 覆盖写没有旧侧 |
| edit_file 的旧内容不持久化 | crates/bsh/src/caps.rs:5021-5088 | 只能从 args 重建请求形状 |
| tool_diff 从 args 合成 | crates/bsh/src/ui/transcript.rs:727-761 | 不是磁盘 diff |
| TurnBasis::Rewritten | crates/bsh/src/ui/diff_pane.rs:70-97 | 明确承认旧侧未知 |
| load() 非 UTF-8 拒绝 | crates/bsh/src/ui/diff_pane.rs:306-315 | 多格式无入口 |
| AuditView 依赖 loaded window + ring | crates/bsh/src/audit.rs:489-618 | 不是耐久审计 |
| Task attempt 只有 run | crates/bsh/src/extension/tasks.rs:288-311 | 没有 change/review 引用 |
| Task.origin.turn 是 u64 | crates/bsh/src/extension/tasks.rs:279-286 | 位置性身份，不稳定 |
| 主 tab 只有四个 | crates/bsh/src/state.rs:237-249 | 没有 Review |
| 右栏只有 Files/Terminal/Diff/Browser/Extensions/Federation/Tasks | crates/bsh/src/ui/right_panel.rs:330-422 | 没有结构审查表面 |
| Parquet tool schema 无 call/change | crates/bsh/src/history.rs:1357-1365 | 历史记录不能支撑 change history |

### 2.3 搜索结论

在 crates/bsh/src 与 docs/bsh-*.md 中搜索 Review Finding、Verification、Review Approval、ChangeSet、SnapshotProvider 等词：

- 没有 ChangeSet / Change / Revision / SnapshotProvider 的领域类型。
- 没有 Finding / Evidence / Verification / ReviewDecision 的模型。
- 没有 Review 主 tab 或 review service。
- 有 approval 语义的地方是 extension package trust、MCP approval、process approval，不是 change review。
- 有 snapshot 语义的地方是 runtime context snapshot、schedule queue snapshot、composition snapshot，不是文件内容快照。

这不是命名问题，是模型缺失。

---

## 3. 能力差距矩阵

评分：🔴 未实现；🟡 部分/可复用；🟢 已满足。

| 维度 | 结构审查 | 多格式 | Agent-aware | 智能验证 | 可审计变更 | GPUI 表面 | Task 联接 |
|---|---|---|---|---|---|---|---|
| 数据模型 | 🔴 | 🔴 | 🟡 | 🔴 | 🟡 | 🔴 | 🔴 |
| 捕获 | 🔴 | 🔴 | 🟡 | 🔴 | 🔴 | — | — |
| 持久化 | 🔴 | 🔴 | 🟡 | 🔴 | 🟡 | — | 🟡 |
| 结构解析 | 🔴 | 🔴 | — | — | — | — | — |
| 渲染 | 🔴 | 🔴 | — | — | — | 🔴 | — |
| 交互 | 🔴 | 🔴 | — | 🔴 | — | 🔴 | 🔴 |
| 测试 | 🔴 | 🔴 | 🟡 | 🔴 | 🟡 | 🔴 | 🔴 |

**结论：** 不是“补几个 UI 控件”的差距，而是 Change/Snapshot/Review 三个领域层缺失。先做 UI 会得到更多“看起来像 review”的静态卡片，无法支撑真实 diff、审批和审计。

---

## 4. Gap Register

### 4.1 P0：阻塞真实 Review

#### G-01 没有 ChangeSet / Change / Revision 领域模型

- **现状：** TurnFile / TurnBasis / PreviewLine 都在 UI 层；没有跨模块的 change identity。
- **证据：** crates/bsh/src/ui/diff_pane.rs:70-140；crates/bsh/src/ui/transcript.rs:777-829。
- **影响：** 无法把一次 turn、一个 task attempt、一个 subagent、一次 audit 行关联到同一变更；无法做 digest、stale、review history。
- **目标：** 按 RFC 第 5 节引入 ChangeSet/Change/Revision/SnapshotRef。
- **关闭条件：** domain 类型不依赖 GPUI；manifest 可 round-trip；digest 稳定。
- **优先级：** P0。

#### G-02 没有稳定 TurnId / CallId

- **现状：** UI 用 message index、turn ordinal、tool array position；Task.origin.turn 是 u64；AuditCall 用 (message, tool_index)。
- **证据：** crates/bsh/src/audit.rs:517-539；crates/bsh/src/extension/tasks.rs:279-286。
- **影响：** 分页、压缩、重载后位置可能变化；review/audit 链接不可靠。
- **目标：** 新 turn 生成 UUIDv7；旧 session 回填 legacy turn id；ToolEvent 增加 call_id。
- **关闭条件：** 跨 reload/paging 的 deep link 测试通过。
- **优先级：** P0。

#### G-03 没有 before-image

- **现状：** write_file 只记录新内容；edit_file 的旧内容只存在于调用栈；覆盖写被标为 Rewritten。
- **证据：** crates/bsh/src/ui/transcript.rs:727-733；crates/bsh/src/ui/diff_pane.rs:70-75；crates/bsh/src/caps.rs:4964-5011。
- **影响：** 无法画出真实 diff；无法证明“改了什么”；review 没有基础事实。
- **目标：** SnapshotProvider 在写前/写后捕获真实字节，写入 content-addressed store。
- **关闭条件：** write_file 覆盖已有文件时，before/after 与磁盘一致；不再显示 Rewritten。
- **优先级：** P0（最高）。

#### G-04 没有 content-addressed SnapshotStore

- **现状：** 没有 blob store、hash 校验、去重、GC；session JSON 只存 args/output。
- **证据：** crates/bsh/src/model.rs:21-40；crates/bsh/src/history.rs:1357-1365。
- **影响：** 无法持久化、去重、校验、回收变更内容；不能支撑 audit。
- **目标：** SHA-256 对象存储，原子 put，读取校验，引用计数/GC。
- **关闭条件：** put/get/dedup/corruption/GC 单测通过；损坏 blob 不被返回。
- **优先级：** P0。

#### G-05 没有精确捕获 seam

- **现状：** 写工具没有 snapshot hook；file_lock 只在工具内部；decorator 在锁外读 before 会产生竞态。
- **证据：** crates/bsh/src/caps.rs:4999-5004、:5073-5078。
- **影响：** 即使有 store，也无法保证 before/after 属于同一次写。
- **目标：** SnapshotProvider 接入 write_file/edit_file/apply_patch；before/after 与 mutate 在同一 per-path lock 内。
- **关闭条件：** 并发写同一路径的测试证明 revision 顺序与磁盘一致。
- **优先级：** P0。

#### G-06 Diff 模型是 UI-local 且格式单一

- **现状：** TurnView/TurnFile/TurnBasis 在 ui/diff_pane.rs；Format 只有 text；非 UTF-8 报 diff.binary。
- **证据：** crates/bsh/src/ui/diff_pane.rs:44-157、:306-315。
- **影响：** domain 无法复用；多格式无法扩展；UI 与存储耦合。
- **目标：** 按 RFC 第 5.4/5.5 引入 ChangeDiff/FormatDiff/StructuralDiff；domain 独立。
- **关闭条件：** text、image、config 至少三种格式走同一 ChangeDiff 接口；UI 只消费 view model。
- **优先级：** P0。

#### G-07 没有 Review 聚合、Finding、Evidence、Verification、Decision

- **现状：** 只有 policy audit 和 tool call；没有 review 对象。
- **证据：** 全仓搜索无 Review/Finding/Verification/Decision 领域类型。
- **影响：** 无法记录“发现了什么、证据是什么、谁批准了什么”；无法做 stale 审批。
- **目标：** 按 RFC 第 5.6 引入 Review 聚合；decision 绑定 ChangeSetDigest。
- **关闭条件：** finding 必须有 evidence 或 NeedsHuman；digest 不匹配时 decision 拒绝写入；reload 后 review 完整。
- **优先级：** P0。

#### G-08 Audit 不可耐久、不是完整变更历史

- **现状：** AuditView 从 loaded messages + 进程内 denial ring 构建；cache key 是 message 形状和 denial 数量。
- **证据：** crates/bsh/src/audit.rs:489-618；crates/bsh/src/ui/audit.rs:41-75。
- **影响：** 重启/换窗口/ring 丢弃后，审计不完整；完整变更历史无法成立。
- **目标：** 保留现有 tool-call 投影；新增 append-only review audit.jsonl；change store 作为变更历史权威。
- **关闭条件：** review decision/verification 重启后可读；audit 页面能链接 change。
- **优先级：** P0（联接）/ P1（完整事件）。

#### G-09 Task attempt 没有 change/review 引用

- **现状：** Attempt 只有 run 字符串；Task 只有 origin.session/turn。
- **证据：** crates/bsh/src/extension/tasks.rs:288-391。
- **影响：** task board 无法显示“这次 attempt 改了什么/审到哪”；review 无法回到 task。
- **目标：** Attempt 增加 change_set_id；review 打开后写 review_id；不把 review 状态混入 TaskStatus。
- **关闭条件：** task → change set → review → audit 的 deep link 点击测试通过。
- **优先级：** P0。

### 4.2 P1：让 Review 可用、可看、可跳

#### G-10 没有结构解析/结构 diff

- **现状：** 只有 kit 的 text diff；bsh 没有直接使用 tree-sitter；没有符号树。
- **证据：** crates/bsh/Cargo.toml 只通过 gpui-kit 的 tree-sitter-languages 特性间接依赖；bsh/src 无 tree_sitter 调用。
- **影响：** 无法实现 Structural Review；只能看行变化。
- **目标：** StructuralDiff + adapter；首版 Rust/TS/JS/Python/JSON/TOML/YAML；失败显式 fallback。
- **关闭条件：** 固定 fixture 的 node/move 输出稳定；fallback 不丢 text diff。
- **优先级：** P1。

#### G-11 没有 Review tab / GPUI Structural Renderer

- **现状：** 主 tabs 只有 Chat/Trajectory/Audit/Extensions；右栏 Diff 是 kit 比较器。
- **证据：** crates/bsh/src/state.rs:237-249；crates/bsh/src/ui/right_panel.rs:330-422。
- **影响：** 没有结构树、findings、evidence、decision 的工作面。
- **目标：** Tab::Review + ReviewViewModel + tree/detail/findings；render 不读盘。
- **关闭条件：** 点击/展开/过滤/approve 测试；1k rows 性能测试；空状态完整。
- **优先级：** P1。

#### G-12 多格式支持缺失

- **现状：** 非 UTF-8 直接 diff.binary；没有 image/PDF/docx/archive/config adapter。
- **证据：** crates/bsh/src/ui/diff_pane.rs:306-315。
- **影响：** 截图中的 Multi-format Review 不能成立。
- **目标：** FormatDiff adapter；每种格式明确 supported/partial/fallback；不支持的格式不伪造。
- **关闭条件：** 图片显示尺寸/像素差异；PDF 显示页数/可提取文本；archive 显示 entry 增删；config 显示 key diff。
- **优先级：** P1。

#### G-13 Chat changed-files card 仍是 request-shaped

- **现状：** turn_changes 从 args 合成 +/-；Chat 卡片和 Diff pane 读同一份请求形状。
- **证据：** crates/bsh/src/ui/transcript.rs:795-829。
- **影响：** 用户看到的“变更”与实际磁盘可能不一致，尤其 write_file 覆盖。
- **目标：** 有 ChangeSet 时读真实 change；无 ChangeSet 时明确 legacy/request preview。
- **关闭条件：** 新 session 卡片与磁盘一致；旧 session 显示 legacy 标签。
- **优先级：** P1。

#### G-14 没有 stale decision 机制

- **现状：** 没有 decision；没有 digest；没有 stale 状态。
- **影响：** 审批可能绑定到过期内容，audit 无法证明“批准的是哪一版”。
- **目标：** ReviewDecision.digest；review.digest 与 changeset.digest 比较；不一致 Stale。
- **关闭条件：** 修改 change set 后旧 approval 变 stale；新 decision 必须用新 digest。
- **优先级：** P1（依赖 P0）。

#### G-15 Audit 与 Review 没有联接

- **现状：** AuditCall 只有 tool/target/outcome；没有 change_ids；没有 review 链接。
- **证据：** crates/bsh/src/audit.rs:175-228、:338-351。
- **影响：** 从审计行不能跳到“改了什么/谁审的”。
- **目标：** AuditCall.change_ids；AuditTurn.change_sets；Review 链接；Audit 过滤 Decisions。
- **关闭条件：** audit → review → change 的双向点击测试。
- **优先级：** P1。

#### G-16 Task board 没有 review 状态

- **现状：** Task row 只显示 task status/tier/budget；没有 review badge。
- **证据：** crates/bsh/src/ui/right_panel.rs 的 task row 逻辑（TaskStatus / Tier）。
- **影响：** 长任务完成后，读者不知道是否有待审变更。
- **目标：** review_badge；task detail 显示 change count/open review；不改变 TaskStatus。
- **关闭条件：** Succeeded + Review pending 可同时显示；点击打开 Review。
- **优先级：** P1。

#### G-17 没有 review 导出/重载验收

- **现状：** 有 audit Markdown export；没有 review export；没有 review reload 测试。
- **影响：** 无法把 review 结果交给外部；无法证明持久化。
- **目标：** review Markdown/JSON export（默认不含 blob）；reload 测试。
- **关闭条件：** export 与页面一致；digest/decision 可复核。
- **优先级：** P1。

#### G-18 没有 deep link / focus state

- **现状：** Audit 有 jump_to_audit_turn；Task 有选中；没有跨 tab 的 ReviewFocus。
- **影响：** 三个表面只能各自看，不能形成工作流。
- **目标：** ReviewFocus { task, attempt, changeset, change, finding }；Task/Audit/Review 复用。
- **关闭条件：** 从任一表面跳转后保持选中；返回不丢状态。
- **优先级：** P1。

### 4.3 P2：完整性与规模化

#### G-19 shell/process 效果不可归因

- **现状：** TurnView::ran_commands 只能提示“命令可能改文件”；没有 baseline/scan。
- **证据：** crates/bsh/src/ui/diff_pane.rs:235-256。
- **影响：** 通过 bash 写文件的变更不会出现在 review；用户可能误以为完整。
- **目标：** 默认 Unattributed；可选 observed baseline/scan；显示 Observed 并保留不确定性。
- **关闭条件：** 默认不伪造；开启 observed 后能列出 changed paths 且标“可能包含无关修改”。
- **优先级：** P2（但文案必须现在诚实）。

#### G-20 Snapshot GC / retention / 加密 / secret 处理

- **现状：** 没有对象存储，因此也没有 GC/retention/secret policy。
- **影响：** 一旦上线会无限增长或泄露敏感内容。
- **目标：** 引用计数 GC、30 天 retention、0600/0700、secret 预览遮罩、可选 Vault 加密。
- **关闭条件：** GC 不删除被引用对象；权限测试；secret 默认遮罩。
- **优先级：** P2（必须在 P0 store 落地时一起设计，不能事后补）。

#### G-21 没有性能基准

- **现状：** 现有 Audit cache 有性能纪律，但没有 review 的 1k files/10k nodes 基准。
- **影响：** 大 changeset 可能卡住 UI。
- **目标：** view model build、tree render、lazy file expand 的 benchmark；render 零 I/O 断言。
- **关闭条件：** 基准进入 CI 或至少 nightly；回归可见。
- **优先级：** P2（P1 前建立基线）。

#### G-22 没有远端/多人 review

- **现状：** 单机；federation 没有 review 语义。
- **影响：** 不影响首个版本；但不要在 UI 暗示多人。
- **目标：** 留给后续 RFC；本 RFC 只定义本地聚合。
- **优先级：** P2/Out of scope。

#### G-23 没有 extension host seam

- **现状：** extension platform 有 capabilities/tools/panels，但没有 review service。
- **影响：** 第三方 verification/review 扩展只能走私有路径，违反 extension-first。
- **目标：** ReviewService trait + typed DTO；extension 通过 host 注册。
- **关闭条件：** 一个示例 extension 可以提交 finding/evidence，不触碰 store。
- **优先级：** P2（P1 后）。

#### G-24 没有 tamper-evident audit chain

- **现状：** 没有 review audit log，更没有 prev_hash/hash。
- **影响：** 无法证明 decision 历史未被改写。
- **目标：** append-only audit.jsonl + hash chain；不承诺对抗 root。
- **关闭条件：** 篡改中间行可检测；正常追加可验证。
- **优先级：** P2。

---

## 5. 关键风险

| 风险 | 触发条件 | 影响 | 缓解 |
|---|---|---|---|
| **捕获竞态** | before 在锁外读 | diff 把并发写算到当前调用 | before/after 必须在同一 per-path lock 内 |
| **性能回退** | 每轮扫全 workspace / 每帧读盘 | 直接违反 BSH gold requirement | 文件工具 exact；shell 默认 off；view model 缓存 |
| **存储膨胀** | 每次写都存全文件，无 GC | 磁盘快速增长 | content-addressed 去重 + retention + GC |
| **敏感内容** | 快照包含 secret | 本地泄露/误导出 | policy 过滤、0600、默认遮罩、导出不含 blob |
| **伪完整性** | shell 变更被当成 exact | review 漏看真实变化 | Unattributed/Observed 明确标注 |
| **stale approval** | review 期间内容变化 | 批准了没看过的版本 | decision 绑定 digest；UI stale |
| **依赖膨胀** | 为多格式引入重库 | 编译/包体/安全成本 | adapter 分 feature；首版只做高价值格式 |
| **迁移破坏** | 旧 session 无字段 | 加载失败/静默丢 session | serde default；legacy 回填；round-trip 测试 |
| **Task/Review 耦合** | 把 review 状态写进 TaskStatus | 状态机爆炸 | 两个聚合，badge 只读投影 |
| **UI 先行** | 先画卡片后补数据 | 假功能 | 以 acceptance 为门；P0 前不宣称 |

---

## 6. 可复用与必须新建

### 6.1 直接复用

- 原子写、per-path lock、policy 检查（caps.rs）。
- kit Diff / DiffState / 语法高亮（diff_pane.rs）。
- Audit 的确定性风格与缓存纪律（audit.rs、ui/audit.rs）。
- FileTasksStore、Attempt、RunsService。
- SHA-256（sha2）。
- GPUI 测试基建（debug_selector、click tests）。
- locale 体系（locales/en.yml、zh.yml）。

### 6.2 必须新建

- ChangeSet / Change / Revision / SnapshotRef / Digest 领域类型。
- SnapshotProvider + SnapshotStore + GC。
- ChangeSetStore（manifest、review、audit.jsonl）。
- DiffEngine（text / structural / format adapters）。
- ReviewService（Finding/Evidence/Verification/Decision）。
- Tab::Review + ReviewViewModel + GPUI tree/detail/findings。
- Task/Review/Audit 的 ID 联接和 deep link。
- Review 的 export、reload、stale、performance 测试。

### 6.3 不应新建

- 不要另起一个 UI-only diff 模型。
- 不要把 snapshot 存进 ChatMessage / session JSON。
- 不要重写 kit Diff。
- 不要用 LLM 生成“验证通过”标签。
- 不要为了“多格式”在 P0 引入所有解析库。

---

## 7. 建议路线图

### M0：Domain + Store（P0）

- 类型、digest、manifest、SnapshotStore、feature flag。
- 不接线 UI。
- 出口：类型 round-trip；digest 稳定；旧 session 可加载；store corruption 测试。

### M1：Exact File Capture（P0）

- write_file / edit_file / apply_patch 接入；Diff pane 读真实 change。
- 出口：覆盖写不再 Rewritten；create/edit/patch 有真实 diff；并发测试。

### M2：Text Diff + Chat/Diff 投影（P1 前置）

- TextDiff、legacy fallback、Chat changed-files card 改用 change。
- 出口：新 session 卡片与磁盘一致；旧 session 标 legacy。

### M3：Structural + Review UI（P1）

- tree-sitter adapters、Review tab、finding/evidence/decision。
- 出口：Rust/TS/JS/Python 结构树；approve 绑定 digest；空状态完整。

### M4：Task/Audit 联接（P1）

- Attempt.change_set_id；AuditCall.change_ids；board badge；deep link。
- 出口：task ↔ review ↔ audit 点击链路；reload 后仍可跳转。

### M5：Multi-format（P1/P2）

- image/PDF/docx/archive/config adapters。
- 出口：非 UTF-8 不再一律报错；每格式有明确 fallback。

### M6：Observed Shell（P2）

- bounded baseline/scan；Observed 标注；默认仍 Unattributed。
- 出口：不伪造 exact；性能预算不破。

---

## 8. 验收与测试缺口

| 测试域 | 当前 | 需要新增 |
|---|---|---|
| Snapshot | 无 | put/get/dedup/corruption/GC、并发、路径穿越 |
| Diff domain | 有 UI diff 测试 | digest、state machine、legacy、format adapter |
| Capture | 无 | write/edit/patch 的 before/after 与磁盘一致 |
| Review | 无 | finding/evidence/decision、stale、reload |
| Task | 有 task store 测试 | attempt.change_set_id、review link |
| Audit | 有 audit 测试 | call_id/change_ids、review decision 事件 |
| UI | 有 click tests | Review tree、approve、stale、empty、deep link |
| Performance | 有 audit cache 测试 | 1k files/10k nodes、render 零 I/O |
| Export | 有 audit export | review export、digest 可复核 |

**必须补的负向测试：**

- before 在锁外读导致并发写被误归因（应失败）。
- 没有 before image 时 UI 仍显示“diff complete”（应失败）。
- decision 在 digest 变化后仍可写入（应失败）。
- shell 变更被标 exact（应失败）。
- blob 损坏仍返回字节（应失败）。
- task 成功自动把 review 标 approved（应失败）。

---

## 9. 现在就能做 / 必须等

### 现在就能做（不依赖 P0）

- 把现有 Diff/Audit 的文案改得更诚实：明确“request preview”“captured before/after 尚未实现”。
- 在 design doc / UI 中标注 legacy、Unattributed，不把命令副作用算进文件列表。
- 为 P0 建立性能基线：snapshot read/hash/put、review view model build。

### 必须等 P0

- 任何“真实 diff”“Structural Review”“Multi-format Review”UI。
- 任何 approval 按钮。
- 任何 “Audited Changes” 对外宣称。
- 任何 task board 的 review badge。

### 不建议做

- 先画截图里的五张能力卡，再补后端。
- 用 LLM 生成 finding 当作 verification。
- 把 shell 的 git diff 当作完整 ChangeSet。
- 把 review 状态塞进 TaskStatus。

---

## 10. 待决问题

1. P0 的 TurnId 回填策略是否接受“首次保存时写回”？
2. Snapshot store 是否复用 Vault 加密，还是先明文 + 0600？
3. Review tab 放主列还是右栏？本文推荐主列。
4. 多格式首批做哪些：image + config + markdown 是否足够？
5. PDF/docx 用纯 Rust 还是系统库？需要依赖评估。
6. Shell observed 是否作为 workspace setting，默认是否永不开启？
7. Review decision 是否允许解除 Task Blocked？推荐不自动。
8. 是否允许 review 导出原始 blob？推荐默认不允许。

---

## 11. 结论与建议

**建议按以下顺序执行：**

1. **先做 P0 Domain Model**：没有 ChangeSet/Change/Revision，任何 Review UI 都是沙上建塔。
2. **紧接着做 P0 Snapshot Provider**：没有 before-image，真实 diff/审计无法成立。
3. **再做 P1 Structural Renderer**：在真实 change 上做结构树，才有意义。
4. **最后做 Task/Review/Audit 联接**：用稳定 ID 把三个已有表面接起来，而不是重写它们。

**不要做的事：**

- 不要把当前 tool-request preview 称为 diff。
- 不要把 policy audit 称为 verification。
- 不要把 task 完成称为 review approved。
- 不要在没有 evidence 的 UI 上显示 Verified。

**判断标准很简单：**

> 当用户问“这次到底改了什么、为什么、谁批准的”，系统能否给出 before/after、结构、证据、决定和审计链？

在 P0 完成前，答案是否定的。P0 完成后，答案开始成立；P1 完成后，答案才可用、可看、可跳转。
