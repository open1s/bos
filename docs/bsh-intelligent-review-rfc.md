# BSH Intelligent Review 架构 RFC

## 从 Tool Request Diff 到可审计的 Change Review

- **Status:** Proposed
- **Version:** 0.1.0
- **Date:** 2026-11-09
- **Baseline:** BSH working copy slkkupuqztnx
- **Scope:** 单机 BSH 桌面端的变更捕获、结构审查、Review 领域、GPUI 渲染与 Task/Audit 联接
- **Audience:** BSH/BOS maintainers、GPUI shell engineers、agent/runtime engineers、security reviewers
- **关系:** 本 RFC 独立于 [BSH 2.0 Architecture RFC](bsh-2.0-architecture-rfc.md)。它不依赖 federation、composition engine 或远程节点，可以在单机 BSH 内独立交付。
- **Status update:** 2026-11-09 的首个 MVP 已落地；本文件保留完整目标架构，未完成项在下面的实现状态表里明确标出。

---

## 0. 实现状态（2026-11-10）

| 工作包 | 状态 | 落点 |
|---|---|---|
| P0 ChangeSet / Change / Revision / Evidence 类型 | 已实现 | crates/bsh/src/review/domain.rs |
| P0 content-addressed SnapshotStore | 已实现（无 GC） | crates/bsh/src/review/store.rs |
| P0 write_file 精确 before/after | 已实现 | crates/bsh/src/caps.rs |
| P0 edit_file 精确 before/after | 已实现 | crates/bsh/src/caps.rs |
| P0 apply_patch 逐文件精确 before/after | 已实现 | crates/bsh/src/caps.rs |
| P0 失败/缺失快照的显式状态 | 已实现 | SnapshotState / UnavailableReason |
| P1 文本 diff | 已实现 | crates/bsh/src/review/diff.rs |
| P1 结构 diff（Rust/TS/TSX/JS/Python） | 已实现（含 method -> impl 父节点缩进） | tree-sitter adapters |
| P1 配置 diff（JSON/TOML/YAML） | 已实现 | config key flatten |
| P1 Markdown heading diff | 已实现 | section nodes |
| P1 GPUI Review tab | 已实现（含完整 Diff 打开、finding 跳转、验证区、工具归属） | crates/bsh/src/ui/review.rs |
| P1 digest-bound Approve / Request changes | 已实现 | ReviewStore::decide |
| P1 Task attempt -> change set | 已实现 | Attempt.change_set_id |
| P1 Task / Audit -> Review 链接 | 已实现（Task 精确到 attempt；Audit 打开当前会话最新 Review） | right_panel.rs / audit.rs |
| P1 Audit 显示 review decisions | 已移除 | 决定由 Review tab 承载；Audit 的决策 strip 占高度过多，已删 |
| P2 验证证据生产（测试/命令 -> Verification） | 未实现 | 目前只有验证区与空状态，没有写入者 |
| P2 decision rationale 输入 | 未实现 | 模型/审计已有 rationale；Review 按钮仍写空字符串 |
| P2 shell observed capture | 未实现 | 仍为 Unattributed |
| P2 image/PDF/docx/archive 语义 diff | 部分 | image 显示尺寸；PDF/docx/archive still metadata-level |
| P2 Snapshot GC / encryption / retention | 未实现 | 后续 |
| P2 extension host review service | 未实现 | 后续 |
| P2 tamper-evident audit chain | 未实现 | 后续 |

验证：review 模块 13 个单测（含 created patch header、method parent）、write_file 精确捕获集成测试、state 生命周期与任务链接测试、Review tab 点击/渲染测试、Review -> full Diff 测试均通过；详见下面的验收标准。

Diff/Review 架构与 feature matrix 的逐项审计见 [bsh-diff-architecture-audit.md](bsh-diff-architecture-audit.md)。

---

## 1. 摘要

BSH 已经有三块真实且可用的表面：

1. **Diff pane**：能打开 patch、conflict 文件和普通文件，也能列出某一轮 tool call 写入的文件。
2. **Audit tab**：从已持久化的 ToolEvent 和 policy denial ring 构建确定性的调用/拒绝审计。
3. **Tasks**：有持久化的 task、attempt、run、scheduler 和 board。

但它们不是同一个变更模型：

- Diff 是 **tool request 的形状**，不是磁盘上真实发生的变化。write_file 只记录新内容，edit_file 只记录 old/new 字符串，bash 的副作用完全不可见；现有代码明确写着 “bsh keeps no before-image”（crates/bsh/src/ui/transcript.rs:727、crates/bsh/src/ui/diff_pane.rs:70）。
- Audit 是 **loaded session window + 进程内 denial ring** 的投影，不是可长期保存的变更/决定历史（crates/bsh/src/audit.rs:489）。
- Task 只有 origin.session / origin.turn 和 attempt.run，没有 change set 或 review 的稳定引用（crates/bsh/src/extension/tasks.rs:279、:288）。
- 没有 Review 聚合：没有 Finding、Evidence、Verification、Decision 这些对象。代码库里搜不到对应模型。

这导致截图里的五项能力无法被诚实地宣称：

| 能力 | 当前现实 |
|---|---|
| Structural Review | 只有文本行级 patch；没有符号/结构树 |
| Multi-format Review | 非 UTF-8 直接 diff.binary 拒绝；没有 image/PDF/docx/archive adapter |
| Agent-aware Review | 有 tool/session/task 线索，但没有 change id，无法稳定关联 |
| Intelligent Verification | 只有 policy audit；没有 finding/evidence/verification/approval 模型 |
| Auditable Changes | 有 tool-call audit；没有 content-addressed change history |

**本 RFC 的核心决策是：把“变更”提升为一等领域对象，Snapshot 是权威，Diff 是派生物，Review 是独立聚合，Task/Audit 只通过稳定 ID 联接。**

四层结构：

~~~text
Agent tools / shell / tasks
        |
        v
Change Capture (Snapshot Provider)  --->  content-addressed object store
        |
        v
ChangeSet Store (Change / Revision / Digest)
        |
        v
Diff Engine (text + structural + multi-format adapters)
        |
        v
Review Aggregate (Finding / Evidence / Verification / Decision)
        |
        v
GPUI Review surface, Chat cards, Task board, Audit, Export
~~~

优先级与交付顺序：

| 优先级 | 工作包 | 产出 |
|---|---|---|
| P0 | Diff Domain Model | ChangeSet / Change / Revision / SnapshotRef / Diff / Review / Finding 的类型、不变量、持久化格式 |
| P0 | Snapshot Provider | 精确 before/after 捕获、content-addressed store、不可用原因、GC |
| P1 | GPUI Structural Renderer | Review tab、结构树、懒加载 diff、finding/evidence 面板 |
| P1 | Task / Review / Audit 打通 | task attempt ↔ change set ↔ review ↔ audit event 的稳定联接 |

---

## 2. 背景与现状证据

### 2.1 现有数据流

| 事实 | 代码位置 | 含义 |
|---|---|---|
| ToolEvent 只有 name/args/output/ms/at | crates/bsh/src/model.rs:21-40 | 没有 call id、change id、before/after hash |
| write_file 返回 path/bytes/unchanged | crates/bsh/src/caps.rs:4964-5011 | 不返回旧内容或快照 |
| edit_file 在内部读旧文件再替换 | crates/bsh/src/caps.rs:5021-5088 | 旧内容只在调用栈里，没有持久化 |
| turn_changes 从 tool args 合成 +/- | crates/bsh/src/ui/transcript.rs:795-829 | 是“请求的形状”，不是磁盘 diff |
| TurnBasis::Rewritten 明确承认旧侧未知 | crates/bsh/src/ui/diff_pane.rs:70-97 | 覆盖写无法画真实 diff |
| Diff pane 的 load() 非 UTF-8 返回 diff.binary | crates/bsh/src/ui/diff_pane.rs:306-315 | 多格式审查没有入口 |
| Audit 从 Session.messages + denial ring 构建 | crates/bsh/src/audit.rs:489-618 | 依赖内存窗口和进程内 ring |
| Audit cache 以 message 形状/denial 数量为 key | crates/bsh/src/ui/audit.rs:41-75 | 性能纪律好，但不是持久审计 |
| Task attempt 只有 run 字符串 | crates/bsh/src/extension/tasks.rs:288-311 | 没有 change set 引用 |
| Task.origin 只有 session + turn | crates/bsh/src/extension/tasks.rs:279-286 | turn 是位置性的，不是稳定 ID |
| Main tabs 只有 Chat/Trajectory/Audit/Extensions | crates/bsh/src/state.rs:237-249 | 没有 Review 表面 |
| Right tabs 没有 Review/Diff 之外的结构审查 | crates/bsh/src/ui/right_panel.rs:330-422 | Diff 仍是 kit 的低层比较器 |
| History Parquet tool schema 只有 index/name/args/output/ms | crates/bsh/src/history.rs:1357-1365 | 没有 at、call id 或 change 引用 |

### 2.2 现有实现中值得保留的部分

这不是从零重写。以下能力应复用：

- **原子写与 per-path lock**：write_bytes / file_lock 已经解决并发写同一文件的问题（crates/bsh/src/caps.rs:4999-5004、:5073-5078）。Snapshot 捕获必须复用同一把锁。
- **policy 前置检查**：写工具在读取/写入前先问 policy（crates/bsh/src/caps.rs:4991-4998、:5065-5072）。Snapshot 不能绕过它。
- **确定性 audit 的风格**：不调用模型、所有数字来自记录、缓存 key 明确（crates/bsh/src/audit.rs:1-23、crates/bsh/src/ui/audit.rs:41-75）。
- **GPUI kit 的 Diff 元素**：统一/并排、conflict resolution、行号、语法高亮已经可用（crates/bsh/src/ui/diff_pane.rs:15-18）。Review 不应该重写这个渲染器，而应把“真实 ChangeSet”喂给它。
- **Task store / run seam**：FileTasksStore、Attempt、RunsService 已经是持久、可观察的（crates/bsh/src/extension/tasks.rs:644-729、crates/bsh/src/extension/runs.rs:161-168）。
- **SHA-256 已在依赖图中**：extension package hashing 已使用 sha2（crates/bsh/Cargo.toml）。Snapshot 不需要引入新的哈希实现。

### 2.3 设计约束

1. **不能每帧读盘。** BSH 的性能要求写在 crates/bsh/AGENTS.md：re-parse per render、full walk per frame、synchronous disk read on input path 都是 defect。
2. **不能伪造 diff。** 没有 before-image 时必须显示 Unavailable 和原因，而不是把请求当磁盘变化。
3. **不能把 Review 状态混进 Task 生命周期。** 一个 task 可以成功但 review 仍 pending；review 通过也不自动等于 task 成功。
4. **不能把 audit 变成可变状态。** 决定和证据必须 append-only，页面只是投影。
5. **不能绕过 extension-first。** 新的 host seam 应以 typed service/event 暴露；核心 UI 可以消费，但不能让 extension 依赖私有 shortcut。

---

## 3. 目标与非目标

### 3.1 目标

1. 为每一次 agent 产生的文件/文档变更建立稳定身份。
2. 捕获真实 before/after 内容，支持查看、比较、重放和审计。
3. 用结构化模型表达代码、配置、文档、图片、PDF、压缩包的变更。
4. 把 Review 做成独立聚合：Finding、Evidence、Verification、Decision。
5. 让 Task、Review、Audit 通过稳定 ID 互相跳转，而不是靠 message index。
6. 在 GPUI 中提供高性能的结构审查表面，支持大 changeset。
7. 对无法精确归因的变化（尤其 shell 副作用）明确标注，不假装完整。

### 3.2 非目标

- 不替代 Git，不做完整 VCS 历史、blame、merge queue。
- 不做跨节点变更复制或多人实时协作；这些留给 BSH 2.0 federation。
- 不要求所有语言/格式在第一版都有结构 diff；文本 fallback 必须显式。
- 不自动批准变更，不用 LLM 的判断替代人类 decision。
- 不把 snapshot 内容默认发送给模型或网络。
- 不在本 RFC 内实现；本 RFC 定义目标架构和落地顺序。

### 3.3 设计原则

| 原则 | 具体要求 |
|---|---|
| **Snapshot is truth** | ChangeSet / Revision 的 before/after 快照是权威；diff 可重建、可丢弃 |
| **Diff is derived** | 持久化 diff 只是 cache；格式升级后可以重新生成 |
| **Explicit unknown** | 没有 before、超过阈值、shell 未归因，都必须是显式状态 |
| **Stable identity** | 不依赖 message index、turn ordinal、tool array position |
| **Append-only decisions** | Review decision 和 Audit event 只追加，不修改 |
| **Bind decisions to content** | 审批绑定 ChangeSetDigest；内容变了，审批自动 stale |
| **Measure first** | 结构树、行数、文件数都有上限；渲染前完成，渲染中不做重活 |
| **One source per fact** | Task 状态、Review 状态、Audit 状态各自拥有，不互相复制 |

---

## 4. 总体架构

### 4.1 组件图

~~~text
+-----------------------------+       +-----------------------------+
| Agent tools / Task attempts |       |  Terminal / shell / jobs    |
| write_file, edit_file,      |       |  arbitrary process effects  |
| apply_patch, extensions     |       |                             |
+--------------+--------------+       +--------------+--------------+
               |                                     |
               |  exact mutation boundary            |  observed / unattributed
               v                                     v
+-----------------------------------------------------------------------+
|                        Change Capture Layer                           |
|  SnapshotProvider: before/after, scope, policy, file locks, seal      |
+----------------------------------+------------------------------------+
                                   |
                                   v
+-----------------------------------------------------------------------+
|                     Content-addressed Object Store                    |
|  sha256 blob, dedup, atomic put, bounded size, GC                     |
+----------------------------------+------------------------------------+
                                   |
                                   v
+-----------------------------------------------------------------------+
|                        ChangeSet Store (P0)                           |
|  ChangeSet -> Change -> Revision -> SnapshotRef -> Digest             |
+----------------------------------+------------------------------------+
                                   |
              +--------------------+--------------------+
              |                                         |
              v                                         v
+-----------------------------+           +-----------------------------+
|       Diff Engine           |           |   Review Aggregate (P1)     |
| text / structural / format  |           | Finding / Evidence /        |
| adapters, derived cache     |           | Verification / Decision     |
+--------------+--------------+           +--------------+--------------+
               |                                         |
               +--------------------+--------------------+
                                    |
                                    v
+-----------------------------------------------------------------------+
|                         Projections / UI                              |
| GPUI Review tab, Chat changed-files card, Diff pane, Task board,      |
| Audit tab, Markdown/JSON export                                       |
+-----------------------------------------------------------------------+
~~~

### 4.2 组件职责

| 组件 | 职责 | 不负责 |
|---|---|---|
| **SnapshotProvider** | 在工具边界捕获 before/after；执行 capture policy；seal changeset | 解析 diff、渲染、决定 review 结果 |
| **SnapshotStore** | 内容寻址、去重、原子写入、读取、GC | 业务语义、权限决策 |
| **ChangeSetStore** | 持久化 ChangeSet/Change/Revision；维护 digest | 生成结构树 |
| **DiffEngine** | 从两个 Snapshot 生成 Text/Structural/Format diff | 修改 snapshot、写 review |
| **ReviewService** | 管理 Review、Finding、Evidence、Verification、Decision | 执行工具、改 task 状态 |
| **AuditLog** | append-only 记录决定/证据/策略事件 | 存全量 tool output |
| **UI projections** | 从 view model 渲染，缓存 key 明确 | 在 render 中读盘/解析 |

### 4.3 与现有代码的边界

- **不把 Diff 模型放进 ui/diff_pane.rs。** TurnView/TurnFile/TurnBasis 是 UI 过渡类型；P0 后在 UI 与 domain 之间加映射，domain 不依赖 GPUI。
- **不把 snapshot 塞进 ChatMessage。** ToolEvent 只增加 change_set_id / call_id 的引用，字节在 object store。
- **不重写 AuditView。** 它继续做 tool-call/refusal 的确定性投影；新增的 review 投影是独立类型，二者通过 ID 联接。
- **不重写 Diff kit。** 继续用 DiffFile/DiffState 渲染 text hunks；结构树是它上层的导航模型。

---

## 5. P0：BSH Diff Domain Model

### 5.1 聚合与所有权

~~~text
Session 1---* Turn 1---* ChangeSet 1---* Change 1---* Revision
                                  |                         |
                                  |                         +-- before: SnapshotState
                                  |                         +-- after:  SnapshotState
                                  |
                                  +--* Diff (derived)
                                  +--1 Review 1---* Finding
                                                   +--* Evidence
                                                   +--* Verification
                                                   +--* Decision
Task 1---* Attempt 1---0..1 ChangeSet
AuditEvent *---1 SubjectRef (Session/Turn/ChangeSet/Change/Review/Finding)
~~~

**所有权规则：**

- Turn 拥有交互式轮次的 ChangeSet。
- TaskAttempt 拥有 task 产生的 ChangeSet；如果 task 在某个 turn 内启动，则 ChangeSet.parent 指向该 turn 的 change set。
- Subagent 产生 child ChangeSet，parent 指向父 run 的 change set；Review 默认按根 change set 汇总，但保留 child 明细。
- Review 拥有 findings/evidence/verification/decisions；不拥有 changes。
- AuditEvent 只引用 subject，不复制 subject 内容。

### 5.2 标识类型

所有 ID 都是不透明字符串；不要用 message index 或数组下标作为跨模块身份。

~~~rust
pub(crate) struct ChangeSetId(String);   // UUIDv7, stable across reload/paging
pub(crate) struct ChangeId(String);      // UUIDv7, stable for one resource in a set
pub(crate) struct RevisionId(String);    // UUIDv7, append-only
pub(crate) struct ReviewId(String);      // UUIDv7
pub(crate) struct FindingId(String);     // UUIDv7
pub(crate) struct VerificationId(String);// UUIDv7
pub(crate) struct AuditEventId(String);  // UUIDv7
pub(crate) struct ContentHash(String);   // "sha256:<64 hex>"
pub(crate) struct ChangeSetDigest(String);// "sha256:<64 hex>" over canonical change list
~~~

TurnId 的迁移是 P0 的前置项：

- 新 session：user message 写入时生成 turn_id（UUIDv7），持久化在 message 的可选字段。
- 旧 session：加载时按 session_id + global_message_index 生成一次 legacy id，并在下次保存时回写；global_message_index 来自 loaded_from + local_index，分页/压缩后仍稳定。
- AuditView 仍可先用局部 ordinal 显示，但 domain 层不再依赖它。

### 5.3 核心类型

~~~rust
pub(crate) struct ChangeSet {
    pub(crate) id: ChangeSetId,
    pub(crate) session_id: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) attempt_id: Option<String>,
    pub(crate) parent: Option<ChangeSetId>,
    pub(crate) actor: Actor,
    pub(crate) capture: CaptureMode,
    pub(crate) status: ChangeSetStatus,
    pub(crate) created_at_ms: u64,
    pub(crate) sealed_at_ms: Option<u64>,
    pub(crate) changes: Vec<Change>,
    pub(crate) digest: Option<ChangeSetDigest>,
}

pub(crate) enum Actor {
    Agent { session_id: String, run_id: Option<String> },
    User,
    System,
    Peer { node_id: String },
}

pub(crate) enum CaptureMode {
    Exact,       // 每个 revision 都是工具边界捕获
    Observed,    // 每个 revision 都来自 shell/process 近似
    Mixed,       // exact 与 observed 同时存在
    Legacy,      // 旧 session，无 before-image
}

pub(crate) enum ChangeSetStatus {
    Open,
    Sealed,
    Partial,     // 有些 change 的 snapshot 不可用
    Failed,      // capture 本身失败，不可用于 review
}

// CaptureMode 是 change set 的聚合质量：全部 exact 才是 Exact；全部
// observed 才是 Observed；两者都有是 Mixed；Legacy 仅用于旧记录。
// revision 的精确来源由 SnapshotRef.source 表示。

pub(crate) struct Change {
    pub(crate) id: ChangeId,
    pub(crate) path: ResourcePath,
    pub(crate) kind: ChangeKind,
    pub(crate) revisions: Vec<Revision>,
    pub(crate) first_seen_ms: u64,
    pub(crate) last_seen_ms: u64,
}

pub(crate) struct ResourcePath {
    pub(crate) workspace_relative: String,
    pub(crate) display: String,
    /// For archive members: "bundle.zip!/path/inside".
    pub(crate) container: Option<String>,
}

pub(crate) enum ChangeKind {
    Created,
    Modified,
    Deleted,
    Renamed { from: ResourcePath },
    ModeChanged { from: u32, to: u32 },
    Unchanged,
}

pub(crate) struct Revision {
    pub(crate) id: RevisionId,
    pub(crate) seq: u32,
    pub(crate) operation: ChangeOperation,
    pub(crate) before: SnapshotState,
    pub(crate) after: SnapshotState,
    pub(crate) tool: Option<ToolRef>,
    pub(crate) captured_at_ms: u64,
}

pub(crate) enum ChangeOperation {
    Read,        // 只记录读取，不产生 change；保留给 evidence
    Create,
    Write,
    Edit,
    Patch,
    Delete,
    Rename,
    Mode,
    Observed,
}

pub(crate) struct ToolRef {
    pub(crate) call_id: String,
    pub(crate) name: String,
    /// UI jump hint only; not an identity. Paging may change it.
    pub(crate) message: Option<usize>,
    pub(crate) tool_index: Option<usize>,
}

pub(crate) enum SnapshotState {
    Present(SnapshotRef),
    Absent,
    Unavailable(UnavailableReason),
}

pub(crate) struct SnapshotRef {
    pub(crate) hash: ContentHash,
    pub(crate) bytes: u64,
    pub(crate) media_type: String,
    pub(crate) family: ResourceFamily,
    pub(crate) mode: u32,
    pub(crate) source: SnapshotSource,
}

pub(crate) enum SnapshotSource {
    ToolBefore,
    ToolAfter,
    WorkspaceScan,
    GitIndex,
}

pub(crate) enum ResourceFamily {
    Text,
    Code,
    Config,
    Markdown,
    Image,
    Pdf,
    Document,
    Spreadsheet,
    Archive,
    Binary,
    Unknown,
}

pub(crate) enum UnavailableReason {
    TooLarge { bytes: u64, limit: u64 },
    BinaryNotCaptured,
    PermissionDenied,
    ReadFailed(String),
    OutsideWorkspace,
    CaptureDisabled,
    Raced,
    LegacyNoBeforeImage,
}
~~~

### 5.4 Diff 类型

Diff 是从两个 snapshot 派生的，不进入 ChangeSet 的权威状态。

~~~rust
pub(crate) struct ChangeDiff {
    pub(crate) change_id: ChangeId,
    pub(crate) base: SnapshotState,   // Absent for create
    pub(crate) target: SnapshotState, // Absent for delete
    pub(crate) status: DiffStatus,
    pub(crate) summary: DiffSummary,
    pub(crate) format: FormatDiff,
    pub(crate) generated_at_ms: u64,
}

pub(crate) enum DiffStatus {
    Complete,
    Partial { reason: String },
    Unsupported { reason: String },
}

pub(crate) struct DiffSummary {
    pub(crate) files: usize,
    pub(crate) added: usize,
    pub(crate) removed: usize,
    pub(crate) modified: usize,
    pub(crate) moved: usize,
    pub(crate) binary: usize,
    pub(crate) hunks: usize,
}

pub(crate) enum FormatDiff {
    Text(TextDiff),
    Image(ImageDiff),
    Pdf(PdfDiff),
    Document(DocumentDiff),
    Spreadsheet(SpreadsheetDiff),
    Archive(ArchiveDiff),
    Binary(BinaryDiff),
}

pub(crate) struct TextDiff {
    pub(crate) hunks: Vec<TextHunk>,
    pub(crate) structural: Option<StructuralDiff>,
}

pub(crate) struct TextHunk {
    pub(crate) old_start: u32,
    pub(crate) old_lines: u32,
    pub(crate) new_start: u32,
    pub(crate) new_lines: u32,
    pub(crate) lines: Vec<DiffLine>,
}

pub(crate) struct DiffLine {
    pub(crate) mark: DiffMark, // Context | Added | Removed
    pub(crate) old_line: Option<u32>,
    pub(crate) new_line: Option<u32>,
    pub(crate) text: String,
}

pub(crate) struct ImageDiff {
    pub(crate) old_width: u32,
    pub(crate) old_height: u32,
    pub(crate) new_width: u32,
    pub(crate) new_height: u32,
    pub(crate) changed_bbox: Option<Rect>,
    pub(crate) changed_pixels: Option<u64>,
    pub(crate) perceptual_distance: Option<f32>,
}

pub(crate) struct PdfDiff {
    pub(crate) old_pages: u32,
    pub(crate) new_pages: u32,
    pub(crate) page_deltas: Vec<PageDelta>,
}

pub(crate) struct DocumentDiff {
    pub(crate) old_sections: Vec<SectionRef>,
    pub(crate) new_sections: Vec<SectionRef>,
    pub(crate) changed_sections: Vec<SectionDelta>,
}

pub(crate) struct SpreadsheetDiff {
    pub(crate) sheets: Vec<SheetDelta>,
}

pub(crate) struct ArchiveDiff {
    pub(crate) entries_added: usize,
    pub(crate) entries_removed: usize,
    pub(crate) entries_changed: usize,
    pub(crate) entries: Vec<ArchiveEntryDelta>,
}

pub(crate) struct BinaryDiff {
    pub(crate) old_bytes: u64,
    pub(crate) new_bytes: u64,
    pub(crate) hash_changed: bool,
    pub(crate) note: String,
}
~~~

### 5.5 结构模型

结构模型回答“哪个语义单元变了”，行级 hunk 只作为证据。

~~~rust
pub(crate) struct StructuralDiff {
    pub(crate) adapter: String,          // "tree-sitter-rust@0.23", "json", ...
    pub(crate) language: Option<String>,
    pub(crate) nodes: Vec<StructuralNode>,
    pub(crate) moves: Vec<StructuralMove>,
    pub(crate) confidence: f32,          // 0.0..=1.0
}

pub(crate) struct StructuralNode {
    pub(crate) id: String,               // stable within one diff
    pub(crate) parent: Option<String>,
    pub(crate) kind: StructuralKind,
    pub(crate) qualified_name: String,   // "impl Foo::bar", "server.tls.enabled"
    pub(crate) change: NodeChange,
    pub(crate) old_anchor: Option<Anchor>,
    pub(crate) new_anchor: Option<Anchor>,
    pub(crate) children: Vec<String>,
    pub(crate) detail: String,
}

pub(crate) enum StructuralKind {
    File,
    Module,
    Namespace,
    Import,
    Type,
    Trait,
    Function,
    Method,
    Field,
    ConfigKey,
    Section,
    Page,
    ArchiveEntry,
    ImageRegion,
}

pub(crate) enum NodeChange {
    Added,
    Removed,
    Modified,
    Moved,
    Renamed,
    SignatureChanged,
    Unchanged,
}

pub(crate) struct StructuralMove {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) similarity: f32,
}

pub(crate) struct Anchor {
    pub(crate) byte_start: u64,
    pub(crate) byte_end: u64,
    pub(crate) line_start: u32,
    pub(crate) line_end: u32,
}
~~~

**结构 adapter 规则：**

- adapter 失败时 structural = None，TextDiff 仍然可用。
- adapter 不得发明符号；无法匹配的节点放进 Unchanged 或省略，并在 detail 说明。
- 结构 diff 的 confidence 来自匹配质量（签名/名称/子树相似度），不是模型置信度。
- 语言支持按 adapter 逐个上线；第一版至少 Rust、TypeScript/TSX、JavaScript、Python、JSON、TOML、YAML。

### 5.6 Review 聚合

~~~rust
pub(crate) struct Review {
    pub(crate) id: ReviewId,
    pub(crate) change_set_id: ChangeSetId,
    pub(crate) digest: ChangeSetDigest,
    pub(crate) status: ReviewStatus,
    pub(crate) risk: RiskAssessment,
    pub(crate) findings: Vec<Finding>,
    pub(crate) verifications: Vec<Verification>,
    pub(crate) decisions: Vec<ReviewDecision>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

pub(crate) enum ReviewStatus {
    Draft,
    InReview,
    ChangesRequested,
    Approved,
    Rejected,
    Stale,        // 绑定的 digest 已不是当前 change set
}

pub(crate) struct RiskAssessment {
    pub(crate) level: RiskLevel,       // Low | Medium | High | Critical
    pub(crate) reasons: Vec<RiskReason>,
    pub(crate) requires_human: bool,
}

pub(crate) enum RiskReason {
    PolicySensitivePath,
    BinaryOrOpaqueFormat,
    LargeChange,
    ShellObservedOnly,
    NoVerification,
    FailedVerification,
    SecretLikePath,
    OutsideWorkspace,
}

pub(crate) struct Finding {
    pub(crate) id: FindingId,
    pub(crate) change_id: Option<ChangeId>,
    pub(crate) severity: Severity,     // Info | Low | Medium | High | Critical
    pub(crate) kind: FindingKind,
    pub(crate) status: FindingStatus,
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) source: FindingSource,  // Deterministic | Agent | Human
    pub(crate) evidence: Vec<EvidenceRef>,
    pub(crate) anchor: Option<Anchor>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

pub(crate) enum FindingKind {
    Correctness,
    Security,
    Test,
    Performance,
    Documentation,
    Compatibility,
    DataLoss,
    Policy,
    Other,
}

pub(crate) enum FindingStatus {
    Open,
    Accepted,
    Dismissed,
    Fixed,
    NeedsHuman,
}

pub(crate) struct EvidenceRef {
    pub(crate) id: String,
    pub(crate) kind: EvidenceKind,     // Snapshot | Hunk | Command | Test | Policy | Audit | Note
    pub(crate) label: String,
    pub(crate) snapshot: Option<SnapshotRef>,
    pub(crate) location: Option<Anchor>,
    pub(crate) digest: Option<ContentHash>,
}

pub(crate) struct Verification {
    pub(crate) id: VerificationId,
    pub(crate) kind: VerificationKind, // Build | Test | Lint | Typecheck | Policy | Manual
    pub(crate) status: VerificationStatus, // Passed | Failed | NotRun | Unsupported
    pub(crate) command: Option<String>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) output_digest: Option<ContentHash>,
    pub(crate) summary: String,
    pub(crate) started_at_ms: Option<u64>,
    pub(crate) finished_at_ms: Option<u64>,
}

pub(crate) struct ReviewDecision {
    pub(crate) id: String,
    pub(crate) decision: Decision,     // Approve | Reject | RequestChanges
    pub(crate) actor: Actor,
    pub(crate) rationale: String,
    pub(crate) digest: ChangeSetDigest,
    pub(crate) at_ms: u64,
}

pub(crate) enum Decision {
    Approve,
    Reject,
    RequestChanges,
}
~~~

**为什么 decision 要带 digest：**

- 一个 review 可能持续数分钟，期间 change set 又发生变化。
- 审批必须绑定“当时看到的内容”；用 ChangeSetDigest 而不是 ReviewId。
- UI 打开 review 时比较 review.digest 和 changeset.digest；不一致显示 Stale，并要求重新审阅。
- Audit 可以证明“谁在哪个内容摘要上批准了什么”。

### 5.7 不变量

| # | 不变量 | 违反时的行为 |
|---|---|---|
| I1 | 每个 Change 至少有一个 Revision | 拒绝 seal，标 Failed |
| I2 | 每个 revision 的 before/after 至少一侧是 Present | 否则 Partial，UI 说明原因 |
| I3 | digest 只由排序后的 change/revision 内容计算 | digest 不匹配时 review 变 Stale |
| I4 | Sealed change set 不再接受 revision | 新变化开新 change set 或 child |
| I5 | ReviewDecision.digest 必须等于 review seal 时的 digest | 不相等拒绝写入 |
| I6 | Finding 必须至少有一个 evidence 或明确 NeedsHuman | 空证据 finding 只允许 human 创建 |
| I7 | Audit event 只追加，不改写 | 写入失败即整体失败 |
| I8 | Snapshot hash 校验失败的内容不可读 | 标记 corruption，不返回字节 |
| I9 | UI 不直接读 blob；只读 view model | 违反按性能 defect 处理 |
| I10 | 没有 before image 时不生成“真实 diff” | 只能显示 request preview + legacy 标签 |

### 5.8 生命周期

~~~text
begin(scope)
  -> Open
  -> append revisions (each capture)
  -> seal()
       -> compute digest
       -> write manifest
       -> Sealed / Partial
  -> review.open()
       -> Draft -> InReview
       -> findings/verifications
       -> decision (bound to digest)
       -> Approved / Rejected / ChangesRequested
  -> if digest changes: Stale
~~~

**Seal 时机：**

- 交互式 turn：runner 的 terminal event（成功、失败、取消）之后 seal。
- task attempt：attempt 进入 terminal 状态之后 seal；如果 attempt 被中断，seal 为 Partial 并保留已捕获内容。
- subagent：child run terminal 后 seal；父 change set 继续 open，直到父 turn/attempt 结束。
- 进程崩溃：下次启动扫描 Open change set，按现有 revision 恢复，标记 Interrupted，不猜测缺失内容。

### 5.9 持久化格式

工作区级目录，允许跨 session/task 引用：

~~~text
<workspace>/.bsh/review/
  objects/
    ab/
      abcdef...            # raw bytes, content-addressed
  changesets/
    <changeset-id>.json    # manifest, atomically rewritten
  reviews/
    <review-id>.json       # review aggregate
  audit.jsonl              # append-only review/decision events
  index.json               # schema version, counters, last GC
  .lock                    # store-wide write lock
~~~

**Manifest 版本：**

~~~json
{
  "schema": 1,
  "id": "01J...",
  "session": "6f69...",
  "turn": "01J...",
  "task": null,
  "attempt": null,
  "capture": "exact",
  "status": "sealed",
  "changes": [
    {
      "id": "01J...",
      "path": "crates/bsh/src/audit.rs",
      "kind": "modified",
      "revisions": [
        {
          "seq": 1,
          "operation": "edit",
          "before": {"hash": "sha256:...", "bytes": 8123, "media_type": "text/x-rust", "family": "code", "mode": 420, "source": "tool_before"},
          "after":  {"hash": "sha256:...", "bytes": 8190, "media_type": "text/x-rust", "family": "code", "mode": 420, "source": "tool_after"},
          "tool": {"call_id": "01J...", "name": "edit_file", "message": 42, "tool_index": 3}
        }
      ]
    }
  ],
  "digest": "sha256:..."
}
~~~

**规范化和 digest：**

1. changes 按 workspace_relative 字节序排序。
2. 每个 change 的 revisions 按 seq 排序。
3. 对每个 revision，编码 (path, kind, seq, operation, before-state, after-state)，其中 state 是 absent 或 content hash；不编码 source、时间戳或 UI 状态。
4. 用长度前缀编码后 SHA-256；空 change set 的 digest 也是合法值。
5. digest 不包含 review、capture provenance、UI 状态、时间戳，确保同一内容稳定；capture provenance 作为 risk/UI 信息单独保存。

### 5.10 与现有 ToolEvent / Task / AuditView 的关系

**ToolEvent 增量字段：**

~~~rust
pub(crate) struct ToolEvent {
    // existing fields...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) change_set_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) change_ids: Vec<String>,
}
~~~

- 旧 session 字段缺失时默认 None，不影响加载。
- call_id 在新 turn 开始时生成，写入 tool event；audit/review 不再依赖 (message, tool_index) 作为长期身份。
- change_ids 是冗余索引，便于 UI 快速过滤；权威关系仍在 ChangeSet。

**Attempt 增量字段：**

~~~rust
pub(crate) struct Attempt {
    // existing fields...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) change_set_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) review_id: Option<String>,
}
~~~

- change_set_id 在 attempt 开始时创建或绑定。
- review_id 在 review 打开时写入；不要反向把 review 状态复制到 task status。

**AuditView 联接：**

~~~rust
pub(crate) struct AuditTurn {
    // existing fields...
    pub(crate) change_sets: Vec<String>,
}

pub(crate) struct AuditCall {
    // existing fields...
    pub(crate) call_id: Option<String>,
    pub(crate) change_ids: Vec<String>,
}
~~~

Audit 仍然从 ToolEvent 构建；change/review 只提供链接和摘要，不把 snapshot 字节塞进 audit view。

---

## 6. P0：Snapshot Provider

### 6.1 目标

在工具真正修改文件之前和之后，捕获**真实字节**，并以内容寻址方式持久化。它必须回答三个问题：

1. 这个变更的 before 是什么？
2. 这个变更的 after 是什么？
3. 如果不知道，为什么不知道？

### 6.2 接口

~~~rust
pub(crate) trait SnapshotProvider: Send + Sync {
    fn begin(&self, scope: CaptureScope) -> Result<CaptureSession, SnapshotError>;

    /// Must be called while holding the target's per-path lock.
    fn before_file(
        &self,
        session: &CaptureSession,
        path: &Path,
    ) -> Result<SnapshotState, SnapshotError>;

    /// Must be called while holding the same lock, after the mutation.
    fn after_file(
        &self,
        session: &CaptureSession,
        path: &Path,
        before: SnapshotState,
        tool: ToolRef,
    ) -> Result<Option<ChangeDraft>, SnapshotError>;

    /// Best-effort post-command observation; never claims exactness.
    fn observe_external(
        &self,
        session: &CaptureSession,
        command: &str,
    ) -> Result<Vec<ChangeDraft>, SnapshotError>;

    fn seal(&self, session: CaptureSession) -> Result<ChangeSet, SnapshotError>;
}

pub(crate) struct CaptureScope {
    pub(crate) workspace: PathBuf,
    pub(crate) session_id: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) attempt_id: Option<String>,
    pub(crate) parent: Option<ChangeSetId>,
    pub(crate) actor: Actor,
    pub(crate) mode: CaptureMode,
}

pub(crate) struct ChangeDraft {
    pub(crate) path: ResourcePath,
    pub(crate) operation: ChangeOperation,
    pub(crate) before: SnapshotState,
    pub(crate) after: SnapshotState,
    pub(crate) tool: Option<ToolRef>,
    pub(crate) observed: bool,
}
~~~

### 6.3 精确捕获：文件工具

write_file / edit_file / apply_patch / extension 提供的文件写入工具必须走同一条路径：

~~~text
resolve path
  -> policy check (existing)
  -> per-path lock (existing)
  -> snapshot.before_file()
  -> mutate
  -> snapshot.after_file()
  -> release lock
  -> append revision
~~~

**关键实现约束：**

1. **before 必须在锁内读取。** 如果在锁外读，另一个并发写可以在读和写之间改变文件，diff 就会把别人的变化算到这次调用上。
2. **after 必须在同一锁内读取。** 否则下一次写可能已经覆盖。
3. **失败调用不产生 revision。** 如果工具在 mutate 前失败，不记录 change；如果 mutate 后读取失败，记录 Unavailable(ReadFailed)，不要把请求内容当 after。
4. **unchanged 是有效结果。** write_file 的 byte-identical write 不产生 revision，但可以记录一个 Unchanged evidence（可选）。
5. **apply_patch 逐文件捕获。** 每个文件独立 before/after；部分成功时只记录成功的文件，失败文件标 Unavailable 或省略并说明。
6. **create_file 归一到 write_file 语义。** 已有 UI 代码已经做这个归一（crates/bsh/src/ui/diff_pane.rs:170-186），domain 层应统一，而不是让 UI 继续补洞。

**现有 write_bytes 的行为需要保留：**

- 原子临时文件 + rename；
- byte-identical no-op；
- 失败不留下半写文件；
- 同一路径的 edit/write 串行。

Snapshot 只是在这条路径的两端加读取，不改变写入语义。

### 6.4 shell / terminal / job 的归因

Shell 可以产生任意文件效果，且没有工具级 before/after。必须分三档：

| 档位 | 机制 | UI 声明 |
|---|---|---|
| **Exact** | 文件工具边界 | “Captured before/after” |
| **Observed** | turn/attempt 开始时的 workspace baseline + 结束扫描；只比较 mtime/size/hash | “Observed changes; may include unrelated edits” |
| **Unattributed** | 命令可能写文件，但没有 baseline/扫描权限 | “This command may have changed files; BSH cannot attribute them” |

**建议默认：**

- 文件工具：Exact，始终开启。
- shell：默认 Unattributed，不伪造 diff。
- 用户显式开启 review.deep_capture（或未来 workspace setting）时，才在 turn 开始做 bounded baseline、结束做 scan，标 Observed。
- baseline/scan 使用 ignore walk（与 glob 工具同一引擎），跳过 .git、.bsh、依赖目录、二进制大文件；只记录 path/mtime/size，按需 hash changed files。
- 扫描预算：默认最多 20,000 个 entry、200ms CPU；超预算则 Unattributed，并显示原因。

**为什么不做“全量前后快照”：**

- 大仓库每轮 O(files) 读取会直接违反 BSH 的性能要求。
- 即使扫描，也无法证明某个变化一定由这条命令造成；Observed 必须保留这个不确定性。
- Git 仓库可以额外用 git diff 作为 evidence，但不能替代 ChangeSet。

### 6.5 SnapshotStore

~~~rust
pub(crate) trait SnapshotStore: Send + Sync {
    fn put(&self, bytes: &[u8]) -> Result<SnapshotRef, SnapshotError>;
    fn get(&self, hash: &ContentHash) -> Result<Vec<u8>, SnapshotError>;
    fn has(&self, hash: &ContentHash) -> bool;
    fn gc(&self, referenced: &HashSet<ContentHash>, now_ms: u64) -> Result<GcReport, SnapshotError>;
}
~~~

**写入协议：**

1. 计算 sha256(bytes)。
2. 目标路径 objects/<first2>/<rest>。
3. 若已存在且大小一致，直接复用（dedup）。
4. 写 objects/<first2>/.tmp-<uuid>，fsync，rename 到目标；目录也 fsync（尽力）。
5. 读取时校验 hash 和大小；不匹配返回 corruption，不返回字节。
6. 对象文件权限 0600；目录 0700。

**大小策略：**

| 类型 | 默认上限 | 超限行为 |
|---|---|---|
| 文本/代码/配置 | 16 MiB | Unavailable(TooLarge) |
| 图片/PDF/文档/压缩包 | 64 MiB | 捕获 metadata + hash；正文按需，超限 Unavailable |
| 其他二进制 | 16 MiB | 默认只存 metadata；开启 binary capture 才存 bytes |

**GC：**

- 只删除“没有被任何 change set / review evidence 引用”且超过 retention 的对象。
- GC 在后台线程执行，不在 turn 路径上。
- 先写 tombstone/清单，再删除；崩溃不会留下被引用但已删除的对象。
- 默认 retention 30 天，可配置；review 已批准的 change set 可延长。

### 6.6 失败模式

| 失败 | 行为 |
|---|---|
| 文件太大 | Unavailable(TooLarge)；UI 显示大小和上限，不生成 diff |
| 二进制未开启捕获 | Unavailable(BinaryNotCaptured)；仍显示 hash/size/format |
| 权限拒绝 | Unavailable(PermissionDenied)；与工具 policy 拒绝区分 |
| 路径在 workspace 外 | Unavailable(OutsideWorkspace)；不读取 |
| 读取时文件被替换 | Unavailable(Raced)；不猜测 |
| 磁盘满/写失败 | change set Partial 或 Failed；不阻塞 turn，但 review 必须显示 |
| 旧 session | Legacy + LegacyNoBeforeImage；保留 request preview，但标“不是磁盘 diff” |

### 6.7 安全与隐私

- Snapshot 只存在于本机 workspace 的 .bsh/review，不进入 session JSON、不进入模型 prompt、不默认上传。
- 读取/写入遵守现有 policy 和 workspace confinement；不跟随 symlink 到 workspace 外。
- 复用 audit 的 secret-looking key 检测用于**预览默认遮罩**；但 review 需要看内容时，提供显式 reveal，且不记录到日志。
- objects/ 不进入文件浏览器默认列表（Files pane 应过滤 .bsh）。
- 导出 review 时默认只导出 metadata/diff，不导出原始 blob；导出证据需显式选择。

---

## 7. P1：GPUI Structural Renderer

### 7.1 表面选择

新增主列 Tab::Review，保留右栏 RightTab::Diff：

- **Review tab**：结构树 + 变更详情 + findings/evidence/decision，是审查的主工作面。
- **Right panel Diff**：低层 text/merge 比较器，继续服务 patch、conflict、单文件对比。
- Review 中的 “View raw diff” 打开右栏 Diff，并绑定同一个 ChangeId / revision。
- 不在右栏再塞一棵结构树；右栏宽度不足以同时容纳树和 diff。

### 7.2 数据流

~~~text
ReviewService
  -> ReviewSnapshot { review, changeset, diffs, findings, verifications }
  -> ReviewViewModel::build(snapshot, selection, filters, lang)
  -> GPUI renders rows only
~~~

**规则：**

- ReviewViewModel 在后台/命令式路径构建，构建完成后 cx.notify()。
- render 只读已经构建的 view model；不 fs::read、不 hash、不 parse。
- 缓存 key：(changeset_digest, review_id, filter, lang, selected_node)；selection 变化不应重建整棵树。
- 大 changeset 的 tree 行扁平化，使用 GPUI 的 measured scroll/list；子节点懒展开。
- 文本行只在展开某个文件/结构节点时生成；单文件最多 REVIEW_MAX_ROWS（建议 2,000）行，超出显示 “N more lines”。

### 7.3 View model

~~~rust
pub(crate) struct ReviewViewModel {
    pub(crate) key: ReviewKey,
    pub(crate) header: ReviewHeader,
    pub(crate) rows: Vec<ReviewRow>,
    pub(crate) selected: Option<ReviewSelection>,
    pub(crate) detail: Option<ReviewDetail>,
    pub(crate) findings: Vec<FindingRow>,
    pub(crate) evidence: Vec<EvidenceRow>,
    pub(crate) empty: Option<ReviewEmpty>,
}

pub(crate) struct ReviewHeader {
    pub(crate) title: String,
    pub(crate) task: Option<TaskLink>,
    pub(crate) status: ReviewStatus,
    pub(crate) risk: RiskAssessment,
    pub(crate) digest_short: String,
    pub(crate) stale: bool,
}

pub(crate) struct ReviewRow {
    pub(crate) id: String,
    pub(crate) depth: u16,
    pub(crate) kind: ReviewRowKind, // File | Symbol | Hunk | Finding | Verification
    pub(crate) label: String,
    pub(crate) secondary: String,
    pub(crate) change: NodeChange,
    pub(crate) expandable: bool,
    pub(crate) expanded: bool,
    pub(crate) anchor: Option<Anchor>,
}

pub(crate) enum ReviewEmpty {
    NoChanges,
    LegacyNoBeforeImage,
    SnapshotUnavailable(UnavailableReason),
    UnsupportedFormat { family: ResourceFamily },
    CaptureFailed,
    NoSelection,
}
~~~

### 7.4 布局

~~~text
+-----------------------------------------------------------------------+
| Review  <changeset>  [task] [audit] [status] [digest] [stale warning] |
+----------------------+-----------------------------+------------------+
| Change tree          | Detail                      | Findings         |
|  file                |  structural summary         |  severity        |
|   symbol             |  unified/split diff         |  evidence        |
|    hunk              |  verification evidence      |  decision        |
+----------------------+-----------------------------+------------------+
| footer: filters, counts, "View raw diff", "Open task", "Open audit"  |
+-----------------------------------------------------------------------+
~~~

- 宽屏（>= 1100px）：三栏。
- 中屏（760–1100px）：两栏，findings 折叠为底部 drawer。
- 窄屏（< 760px）：单栏，tree/detail 用 segmented control 切换。
- 空状态必须说明原因，不画空白树。
- 顶部 header 常驻：task link、audit link、review status、digest、stale 标记。

### 7.5 交互

| 操作 | 行为 |
|---|---|
| 单击 file/symbol | 选中，右侧显示 summary/diff |
| 双击 file | 在右栏 Files 打开文件 |
| 展开 symbol | 懒加载该节点的 hunks/children |
| 点击 hunk | 跳到对应行；右栏 Diff 显示同一 revision |
| “View raw diff” | 打开 RightTab::Diff，绑定 change/revision |
| 点击 finding | 选中 evidence；对应 file/symbol 高亮 |
| “Approve” | 写入 decision；要求 digest 未 stale；可选 rationale |
| “Request changes” / “Reject” | 写入 decision + 可选 finding |
| 搜索 | 文件名、符号名、finding 标题；只过滤已构建的 rows |
| 过滤 | file kind / change kind / severity / verification status |
| “Open task” | 切到 Tasks pane 并选中 task/attempt |
| “Open audit” | 切到 Audit tab 并选中 call/turn |

### 7.6 渲染与性能

- **树**：扁平 Vec<ReviewRow>；折叠状态只改变可见索引，不重建 diff。
- **文本**：按文件懒生成 Vec<DiffLine>；只保留展开文件的 rows。
- **语法高亮**：复用现有 kit 的 language registry；不在 render 内 parse。
- **图片**：用 image crate 解码一次，缓存 GPUI RenderImage；大图缩放后再上传纹理。
- **PDF/docx**：第一版只显示 metadata/section 摘要；页面预览按需，不在 tree 构建时渲染。
- **缓存失效**：change set digests、review update、selection/filter、lang。窗口 resize 只重排，不重建。
- **测试**：用 debug_selector 暴露 review-tree、review-row-<id>、review-approve；点击测试读取 view model 状态。

### 7.7 可访问性与本地化

- 每个 icon-only 控件有 tooltip + accessibility label。
- 所有文案走 locales/{en,zh}.yml；新增 key 同步两份。
- 空状态、错误、stale、unavailable 都有独立 key，不拼接英文句子。
- 键盘：上下移动 selection，Enter 展开/打开，Esc 关闭 detail；不拦截输入框。

---

## 8. P1：Task / Review / Audit 打通

### 8.1 联接图

~~~text
Task --1--> Attempt --0..1--> ChangeSet --1--> Review
  |                              |                |
  |                              +--> Change --*--> Finding
  |                              |                |
  |                              +--> Revision    +--> Evidence
  |                                               +--> Verification
  |                                               +--> Decision
  |
  +--> Review (denormalized for board, optional)
AuditEvent --*--> SubjectRef (Task/Attempt/ChangeSet/Change/Review/Finding)
~~~

### 8.2 Task

**写入：**

- task_create / attempt 开始：创建或绑定 change_set_id。
- task 在 turn 内启动：parent = turn.change_set_id。
- task 的 Attempt 写 change_set_id；review 打开后写 review_id。
- task 结束：seal change set；不因 review 状态改 task status。

**读取：**

- Task board row：review_badge（None / Pending / Approved / ChangesRequested / Stale）。
- Task detail：change count、review status、open review、audit link。
- 不把 review.status 写进 TaskStatus；Succeeded + Review pending 是合法组合。

### 8.3 Review

- Review 的 header 显示 task title/attempt 和 origin turn。
- 从 task 打开 review：ReviewFocus { task_id, attempt_id, change_set_id }。
- 从 review 打开 task：切到 Tasks pane，选中 attempt；不重新加载 task 列表（使用已有 cache）。
- Finding 可以引用 task/attempt；不复制 task 状态。

### 8.4 Audit

**保留现有 AuditView：** tool calls、refusals、policy counts 继续从 ToolEvent + denial ring 构建。

**新增联接：**

- 每个 AuditCall 增加 call_id / change_ids。
- 每个 AuditTurn 增加 change_set_ids。
- Audit 行可显示 “Review” 链接；点击跳 Review 对应 change。
- Review decisions/verifications 写入 audit.jsonl，并出现在 Audit tab 的 “Decisions” 过滤器中。

**耐久性：**

- 现有 denial ring 仍是进程内、bounded；不假装它是完整审计。
- audit.jsonl 只记录 review 生命周期和 decision/evidence 摘要，不记录全量 tool output。
- 如果未来要把 refusal 也持久化，另开 change；不在本 RFC 里混入。

### 8.5 事件类型

~~~rust
pub(crate) enum AuditAction {
    ChangeSetSealed,
    ReviewOpened,
    FindingCreated,
    FindingUpdated,
    VerificationRecorded,
    DecisionRecorded,
    ReviewStaled,
}

pub(crate) struct AuditEvent {
    pub(crate) id: AuditEventId,
    pub(crate) at_ms: u64,
    pub(crate) actor: Actor,
    pub(crate) action: AuditAction,
    pub(crate) subject: SubjectRef,
    pub(crate) detail: serde_json::Value,
    pub(crate) prev_hash: Option<ContentHash>,
    pub(crate) hash: ContentHash,
}
~~~

hash 覆盖 (prev_hash, canonical(detail))，提供轻量 tamper evidence；不是区块链，也不承诺对抗拥有文件系统写权限的攻击者。

### 8.6 命令与 host seam

**Model-facing tools（P1 可选）：**

- review_changes(changeset?) → 返回 review 摘要/IDs。
- review_finding_add(...) → agent 可以提交 finding（带 evidence）。
- review_verify(...) → 记录 verification。
- review_decide(...) → 只允许人类/授权 actor；模型不能自我批准。

**Extension host seam：**

~~~rust
pub(crate) trait ReviewService: Send + Sync {
    fn changeset(&self, id: &str) -> Result<Option<ChangeSetView>, String>;
    fn list_reviews(&self, filter: ReviewFilter) -> Result<Vec<ReviewSummary>, String>;
    fn add_finding(&self, review: &str, finding: FindingDraft) -> Result<FindingId, String>;
    fn record_verification(&self, review: &str, v: VerificationDraft) -> Result<VerificationId, String>;
    fn decide(&self, review: &str, decision: DecisionDraft) -> Result<(), String>;
}
~~~

这符合 extension-first：第三方 review/verification 扩展通过 typed service 读取 change、提交 finding/evidence，不直接碰 store 或 GPUI。

---

## 9. 失败模式与不变量

| 场景 | 期望行为 |
|---|---|
| 文件工具成功但 snapshot 写入失败 | turn 不失败；change set Partial；review 显示 “capture failed”，不生成假 diff |
| 文件工具失败 | 不产生 change；可选记录失败 evidence |
| shell 写文件但未开启 deep capture | Unattributed，Review 显示 “may have changed files” |
| 旧 session 无 before | LegacyNoBeforeImage；保留 request preview，明确标注 |
| review 打开后 changeset 又变化 | digest 不匹配；review Stale；旧 decision 保持历史，但不能作为当前批准 |
| 审批时 digest 不匹配 | 拒绝写入；提示重新审阅 |
| Blob 损坏 | 读取校验失败；UI 显示 corruption；不返回字节 |
| 大文件/大 changeset | 明确截断/上限；不阻塞主线程 |
| 同一文件并发写 | per-path lock 串行；snapshot 与写在同一锁内 |
| task 成功但 review 未完成 | Task 仍 Succeeded；board 显示 review pending |
| review 通过但 task 失败 | Review 仍 Approved；两者独立，UI 同时显示 |

---

## 10. 性能预算

| 路径 | 预算 | 证据方式 |
|---|---|---|
| 文件工具前的 before 读取 | 只读一次，最大 16 MiB；不扫描目录 | tracing span |
| after 读取 | 只读一次；与 before 同一锁 | tracing span |
| object put | 顺序写 + hash；不阻塞 UI | 后台/worker |
| changeset seal | O(changes)；写 JSON | 单元测试 |
| review view model | 1k files / 10k nodes 构建 < 100ms（目标） | benchmark |
| render | 不读盘、不 hash、不 parse；稳定帧 | 帧测试/代码审查 |
| 展开文件 | 最多 2,000 行；超出说明 | UI 测试 |
| shell observed scan | 默认关闭；开启时 20k entries / 200ms 上限 | tracing/测试 |

**明确不接受的实现：**

- 每帧 fs::read 或 serde_json::from_str。
- 每帧重建 tree/findings。
- 在 UI 线程做整仓扫。
- 为了“完整性”在每个 turn 全量读取 workspace。

---

## 11. 迁移与发布

### 11.1 阶段

| 阶段 | 内容 | 退出条件 |
|---|---|---|
| **M0 Domain** | 类型、digest、manifest、store、feature flag；不接线 | 单元测试覆盖不变量；旧 session 仍可加载 |
| **M1 Exact File Capture** | write_file / edit_file / apply_patch 接入；Diff pane 读真实 change | 覆盖写不再显示 Rewritten；create/edit/patch 有真实 diff |
| **M2 Text Diff + Chat/Diff** | Text diff、legacy fallback、Chat changed-files card 改用 change | 旧 session 标 legacy；新 session 卡片与磁盘一致 |
| **M3 Structural + Review UI** | tree-sitter adapters、Review tab、findings/decision | Rust/TS/JS/Python 结构树可展开；decision 绑定 digest |
| **M4 Task/Audit 联接** | task attempt/review/audit 链接；board badge；deep link | 点击链路完整；reload 后仍可跳转 |
| **M5 Multi-format** | image/PDF/docx/archive/config adapters | 非 UTF-8 不再一律报错；每种格式有明确 fallback |
| **M6 Observed Shell** | bounded baseline/scan；Observed 标注 | 默认仍 Unattributed；开启后不伪造 exact |

### 11.2 Feature flags

- review.capture.exact（默认 on）
- review.capture.observed_shell（默认 off）
- review.structural（默认 on，adapter 逐个启用）
- review.ui（默认 on，Review tab）
- 旧 build 读新 session：未知字段忽略；新 build 读旧 session：legacy 模式。

### 11.3 回滚

- 关闭 review.ui：Review tab 隐藏；Chat/Diff 回退到现有 args-based 预览。
- 关闭 review.capture.*：不再写新 change set；已有 review 只读。
- 不删除 .bsh/review；回滚不破坏已有历史。
- Domain schema 版本号在 index.json；不兼容变更走新目录/迁移脚本。

---

## 12. 验收标准

### 12.1 功能

| # | 场景 | 期望 |
|---|---|---|
| A1 | write_file 覆盖一个已有文件 | before/after 真实；diff 是磁盘差异；不再显示 Rewritten |
| A2 | edit_file 替换字符串 | before/after 真实；hunk 行号正确 |
| A3 | apply_patch 多文件 | 每个文件独立 change；部分失败有明确状态 |
| A4 | 创建/删除/重命名 | Created / Deleted / Renamed；重命名显示为 move |
| A5 | 图片变更 | 显示尺寸/格式/像素差异；不再 diff.binary |
| A6 | PDF/docx/archive/config | 至少 metadata + 结构摘要；不支持时明确 fallback |
| A7 | reload | change/review/finding/decision 全部保留；digest 不变 |
| A8 | task attempt | task → change set → review 可跳转 |
| A9 | audit | audit call → change/review 可跳转；decision 出现在 audit |
| A10 | stale | 内容变化后旧 approval 变 stale；新 decision 必须绑定新 digest |
| A11 | legacy session | 显示 “legacy: no before-image”；request preview 仍可见但不算磁盘 diff |
| A12 | shell | 默认 Unattributed；开启 observed 后显示 Observed 且不声称 exact |

### 12.2 测试

**单元：**

- digest canonicalization 稳定、顺序无关、内容敏感。
- SnapshotStore put/get/dedup/corruption/GC。
- SnapshotState 状态机；UnavailableReason 映射。
- Review decision digest 校验；stale 判定。
- 结构 adapter 对固定 fixture 的 node/move 输出。
- 路径穿越、symlink、outside workspace 拒绝。

**集成：**

- write_file/edit_file/apply_patch 的 before/after 与磁盘一致。
- 并发写同一路径：revision 顺序和 lock 一致。
- task attempt seal + review open + decision + audit event。
- 旧 session 加载、保存、再次加载的兼容性。
- 大 changeset（1k files）不阻塞、不 panic。

**UI：**

- Review tab 点击文件/符号/hunk 的状态变化。
- Approve 在 stale 时禁用并说明原因。
- 空状态、unavailable、legacy 各自渲染正确文案。
- 1k rows 的 tree 虚拟化/懒加载。
- Task/Audit deep link 点击测试。

**性能：**

- render 期间无 fs::read/hash/parse（代码审查 + 测试 hook）。
- view model 只按 digest 重建。
- 展开大文件有行数上限。

### 12.3 安全

- Snapshot 不离开本机；不进入 prompt。
- .bsh/review 权限 0700/0600。
- 不跟随 symlink 出 workspace。
- 默认预览遮罩 secret-looking 值；reveal 需显式动作。
- Audit/decision 不可静默改写。

---

## 13. 待决问题

1. **TurnId 的持久化位置**：放在 user ChatMessage 还是 session 的 sidecar？推荐 message 字段，便于 paging 后仍稳定。
2. **Shell deep capture 默认值**：推荐默认 off，避免性能回退；是否提供 workspace setting？
3. **Review tab 还是右栏**：推荐主列 Review tab，右栏 Diff 保留 raw/merge。需要在 UI 评审中确认。
4. **PDF/docx 依赖**：选择纯 Rust extractor 还是系统库？必须先做依赖/性能评估，不在 P0 决定。
5. **Review decision 是否可解除 Task Blocked**：推荐“不自动”；只有显式 approval gate 才改变 task 状态。
6. **Snapshot retention**：30 天是否足够？是否允许用户 pin 已批准的 change set？
7. **是否加密 at rest**：当前工作区已有 Vault；review snapshot 是否复用？P1 前决定。
8. **是否记录 read-only evidence**：读取文件是否进入 change set？推荐只作为 finding evidence，不产生 Change。

---

## 14. 决策摘要

| ID | 决策 | 理由 |
|---|---|---|
| D1 | Snapshot 是权威，Diff 是派生 | 只有真实 before/after 才能支撑 review/audit |
| D2 | 每个执行 scope 一个 ChangeSet | 与 turn/attempt 对齐，支持 seal/digest/stale |
| D3 | 内容寻址对象存储 | 去重、可校验、可 GC、跨 session 复用 |
| D4 | 文件工具 exact，shell observed/unattributed | 不牺牲性能，也不伪造完整性 |
| D5 | Review 独立于 Task 生命周期 | 避免“成功=批准”的错误耦合 |
| D6 | Decision 绑定 ChangeSetDigest | 内容变化后审批自动失效 |
| D7 | 结构 adapter 可插拔，文本 fallback 显式 | 先支持高价值语言，不阻塞其他格式 |
| D8 | Audit 保留现有投影，新增 review event log | 不重写已验证的 audit，同时补齐耐久决定历史 |
| D9 | UI 只读 view model，render 不做重活 | 符合 BSH 性能要求 |
| D10 | Legacy 模式明确标注 | 不把旧 session 的请求预览伪装成磁盘 diff |

---

## 15. 附：与产品能力的映射

| 产品能力 | 本 RFC 的落点 |
|---|---|
| Structural Review | StructuralDiff + tree-sitter adapters + Review tree |
| Multi-format Review | ResourceFamily + FormatDiff adapters（image/PDF/docx/archive/config） |
| Agent-aware Review | ChangeSet 的 session/turn/task/attempt/tool 引用 + stable IDs |
| Intelligent Verification | Finding / Evidence / Verification / Decision + deterministic risk |
| Auditable Changes | content-addressed ChangeSet + digest + append-only audit.jsonl |

这五项能力只有在对应 acceptance 通过后才可以在 UI 中显示“已支持”。在 P0/P1 完成前，现有 Diff/Audit 仍应保持诚实措辞：它们展示的是 tool-call 记录，不是完整变更历史。
