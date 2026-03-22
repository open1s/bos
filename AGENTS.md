# AGENTS.md

## 项目概览

`bos` 是一个 Rust workspace，目标是构建基于 Zenoh 消息总线的多 Agent 框架与示例集合，核心能力包括：

- Agent 生命周期管理（构建、运行、上下文维护）
- A2A（Agent-to-Agent）通信与发现
- Tool 注册、发现与 RPC 调用
- 流式输出与背压处理
- 调度器（workflow/scheduler）能力
- 性能基准测试（criterion + pprof）

工作区主要模块：

- `crates/config`：配置加载与管理
- `crates/bus`：总线抽象与传输层能力
- `crates/agent`：Agent 核心能力（A2A、Tools、Streaming、Session、Scheduler）
- `examples/*`：真实示例（推荐 `examples/llm-agent-demo`）
- `benches/*`：性能基准（agent 与 bus）

## 环境与工具约定（强制）

- 使用 `fish` shell，不使用 bash 语法编写脚本。
- 使用 `mise` 管理多版本工具链（Rust/Node 等）。
- Shell 示例统一使用 fish 语法（例如 `set -x VAR value`）。

建议初始化：

```fish
mise install
mise trust
mise exec -- cargo --version
```

## 快速启动

1. 启动 Zenoh 路由（默认 7447）：

```fish
zenohd
```

2. 启动示例（LLM 双 Agent）：

```fish
cd examples/llm-agent-demo
cargo run --bin bob
```

另一个终端：

```fish
cd examples/llm-agent-demo
set -x OPENAI_API_KEY "your-key"
set -x OPENAI_API_BASE_URL "https://api.openai.com/v1"
set -x OPENAI_MODEL "gpt-4o"
cargo run --bin alice
```

## 常用开发命令

```fish
# 全量检查
cargo check --workspace

# 全量测试
cargo test --workspace

# 运行关键 benchmark（示例）
cargo bench -p agent-benches --bench streaming
cargo bench -p agent-benches --bench rpc_payload
cargo bench -p bus-benches --bench message_serialization
```

## 序列化与性能优化准则（rkyv 优先）

在本项目中，遵循以下原则：

- 传输层与高频路径优先使用 `rkyv`（尤其是 streaming、RPC payload、pubsub 高频消息）。
- 对外兼容或动态结构场景可以保留 `serde_json::Value`，但在传输边界尽量转为字节并采用 `rkyv` 封装。
- 新增消息结构体时，优先评估是否可 `#[derive(Archive, Serialize, Deserialize)]`。
- 避免在热路径频繁 `to_string()/from_str()`；优先 `to_vec()/from_slice()` 或二进制归档。
- 需要保留 JSON 时，优先“业务层 JSON，传输层 bytes”模式。

建议改造优先级：

1. Streaming token/event 批量消息
2. Tool RPC 请求/响应 payload
3. PubSub 高频广播模型
4. Session 持久化（按兼容性评估）

## 编译警告与质量门禁

- 目标：`cargo check --workspace` 无 warning（至少在 CI 目标 profile 下）。
- 新增代码禁止引入可避免的 clippy/rustc warning。
- 性能优化变更需至少补一项 benchmark 或对现有 benchmark 做前后对比记录。

## Agent 协作约定

- 修改前先阅读相关 crate 的 `Cargo.toml` 与 README，优先局部最小改动。
- 不回滚与当前任务无关的现有改动。
- 评审优先级：正确性 > 稳定性 > 性能 > 可读性。
- 如涉及协议/序列化格式变更，必须同步更新：
  - 对应 example（最少一个）
  - 对应测试（单测/集成测）
  - 对应 benchmark（如在热路径）

---

## LLM Agent 编码规范

### 代码风格

- **遵循 `rustfmt` 默认配置**，不自定义 fmt 选项。
- **模块组织**：
  - `src/lib.rs` 仅声明子模块和 public re-export。
  - 每个主要功能使用独立模块（`src/a2a/`, `src/tools/` 等）。
  - `pub(crate)` 用于 crate 内部共享，慎用 `pub`。
- **命名规范**：
  - 类型名使用 PascalCase（`AgentSession`, `ToolDefinition`）。
  - 函数与变量使用 snake_case（`start_agent`, `recv_message`）。
  - 常量使用 SCREAMING_SNAKE_CASE。
  - 避免前缀命名（不用 `AgentAgent`，直接用 `Agent`）。

### 错误处理

- **统一使用 `thiserror` 定义错误类型**：
  ```rust
  #[error("target agent {target:?} not found")]
  pub struct AgentNotFound { target: AgentId }
  ```
- **错误转换**：使用 `?` 操作符传递错误，必要时用 `.context()` 添加上下文。
- **区分可恢复/不可恢复错误**：
  - 可恢复：返回 `Result<T, E>`，允许调用方重试。
  - 不可恢复：使用 `panic!` 或 `abort`（仅在严重状态不一致时）。

### 异步编程

- **统一使用 `async`/`.await`，避免混合阻塞与异步代码**。
- **Tokio runtime**：在异步上下文使用 `tokio::spawn` 时，传递 `Arc<Context>` 而非 clone 大对象。
- **取消安全**：确保所有 `.await` 点可安全取消；使用 `pin_mut!` 或 `Box::pin` 管理生命周期。
- **流式处理**：使用 `futures::StreamExt`，统一处理 backpressure。

### 性能敏感代码

- **热点路径禁止**：
  - 动态派发（`dyn Trait`）在消息处理主循环内。
  - 不必要的 `Arc::clone`/`Rc::clone`。
  - 大对象的非必要 clone（优先 `&T` 或 `Arc<T>`）。
- **内存分配**：
  - 预分配 `Vec`/`String` 容量：`Vec::with_capacity(n)`。
  - 复用 buffer（对象池模式用于高频消息）。
- **锁竞争**：
  - 优先使用 `RwLock` 而非 `Mutex`（读多写少场景）。
  - 细粒度锁 > 粗粒度锁；考虑 `dashmap` 做并发 map。

### 序列化（rkyv 优先）

- **所有内部消息类型**必须派生 `Archive`：
  ```rust
  #[derive(Archive, Serialize, Deserialize, Debug)]
  #[archive_attr(derive(Debug))]
  pub struct AgentMessage { /* ... */ }
  ```
- **版本兼容性**：首次使用 `rkyv` 时开启 `validation` feature，后续变更需保持 ABI 兼容。
- **混合场景**：若需 JSON 输出，实现 `Serialize` 手动转换：
  ```rust
  impl Serialize for AgentMessage {
      fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
      where S: Serializer {
          // rkyv -> serde_json::Value -> ...
      }
  }
  ```

### 依赖管理

- **遵循最小依赖原则**：仅在需要时引入 crate。
- **禁止使用**：
  - `rand` 替代 `getrandom`（或直接用 `rand` 但锁定版本）。
  - 多个日志门面（统一 `tracing`）。
- **Cargo features**：
  - 每个 feature 独立定义，按需组合。
  - `default` feature 仅包含最常用功能。

### API 设计原则

- **面向用户**：API 设计优先考虑调用方便利，而非实现方。
- **零成本抽象**：避免不必要的 runtime 开销。
- **一致性**：相似 API 采用相似签名（如 `send(&self)` 与 `recv(&self)`）。
- **错误信息**：提供可操作的错误信息，避免 "operation failed" 之类泛泛描述。

### 文档要求

- **public API 必须写文档注释**（`///`），说明：
  - 功能用途
  - 参数含义（`/// - id: Agent 唯一标识`）
  - 返回值与错误情况
  - 示例代码（`/// # Example`）
- **复杂模块**在 `src/module/README.md` 中补充设计说明。
- **更新 CHANGELOG**：对用户可见变更添加条目。

### 测试策略

- **单元测试**：与实现同文件（`#[cfg(test)]` mod tests）。
- **集成测试**：`tests/` 目录下的完整场景测试。
- **示例测试**：`examples/*` 中的代码可作为测试目标（`cargo test --example`）。
- **属性测试**：`proptest` 用于关键算法（序列化、编解码）。

### 发布与版本

- **遵循 Semantic Versioning**。
- **变更分类**：
  - `feat:` 新功能（minor）
  - `fix:` 修复（patch）
  - `perf:` 性能优化（patch/major 视影响而定）
  - `BREAKING CHANGE:` 破坏性变更（major）

---

## 特定模式库

### Agent 生命周期

```rust
// 标准模式：Build -> Start -> Run -> Stop
impl Agent {
    pub fn builder() -> AgentBuilder { /* ... */ }

    pub async fn start(&mut self) -> Result<(), AgentError> { /* ... */ }

    pub async fn run(&mut self) -> Result<(), AgentError> { /* ... */ }

    pub async fn stop(self) -> Result<(), AgentError> { /* ... */ }
}
```

### Tool 定义模式

```rust
#[tool]
async fn do_something(input: ToolInput) -> Result<ToolOutput, ToolError> {
    // 验证输入
    let params = input.parse::<MyParams>()?;

    // 业务逻辑
    let result = process(params).await?;

    Ok(ToolOutput::json(result))
}
```

### 事件流模式

```rust
async fn events(&self) -> impl Stream<Item = Event> {
    let (tx, rx) = mpsc::channel(100);

    // 生产者
    tokio::spawn(async move {
        while let Some(event) = producer.next().await {
            if tx.send(event).await.is_err() { break; }
        }
    });

    // 消费者
    rx
}
```

### 错误传播模式

```rust
context链条: UserError → A2A Error → AgentError → MainError
```

---

## Jujutsu/Git 协作规范

- **commit 信息格式**：
  ```
  <type>(<scope>): <subject>

  <body>

  <footer>
  ```
  - type: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `chore`
  - scope: 受影响的模块（如 `a2a`, `tools`, `streaming`）

- **分支策略**：
  - `main`: 稳定可发布分支
  - `feature/*`: 功能开发分支
  - `fix/*`: 修复分支
  - `release/*`: 发布准备分支

- **PR 要求**：
  - 至少一个 Reviewer 批准。
  - CI 通过（`cargo check`, `cargo test`）。
  - 无新警告。
  - 更新相关文档/CHANGELOG。

