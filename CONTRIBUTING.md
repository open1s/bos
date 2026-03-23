# 贡献指南

感谢您对 BrainOS 项目的关注！我们欢迎任何形式的贡献。

## 📋 目录

- [行为准则](#行为准则)
- [如何贡献](#如何贡献)
- [开发流程](#开发流程)
- [代码规范](#代码规范)
- [提交规范](#提交规范)
- [测试](#测试)
- [文档](#文档)

---

## 🤝 行为准则

### 我们的承诺

为了营造开放和友好的环境，我们承诺：
- 尊重不同的观点和经验
- 优雅地接受建设性批评
- 关注对社区最有利的事情
- 对其他社区成员表示同理心

### 不可接受的行为

- 使用性化的语言或图像
- 人身攻击或侮辱性评论
- 公开或私下骚扰
- 未经许可发布他人的私人信息
- 其他不道德或不专业的行为

---

## 🚀 如何贡献

### 报告 Bug

如果您发现了 bug，请：
1. 检查 [Issues](https://github.com/your-org/bos/issues) 确认问题未被报告
2. 创建新的 Issue，包含：
   - 清晰的标题
   - 详细的问题描述
   - 复现步骤
   - 预期行为和实际行为
   - 环境信息（Rust 版本、操作系统等）
   - 相关日志或错误信息

### 提出新功能

如果您有新功能的想法：
1. 先在 [Discussions](https://github.com/your-org/bos/discussions) 讨论
2. 创建 Issue 描述功能需求
3. 说明用例和预期收益
4. 等待维护者反馈后再开始实现

### 提交代码

#### 1. Fork 仓库

```bash
# Fork 并克隆仓库
git clone https://github.com/your-username/bos.git
cd bos

# 添加上游仓库
git remote add upstream https://github.com/your-org/bos.git
```

#### 2. 创建分支

```bash
# 更新主分支
git fetch upstream
git checkout main
git merge upstream/main

# 创建特性分支
git checkout -b feature/your-feature-name
```

分支命名规范：
- `feature/` - 新功能
- `fix/` - Bug 修复
- `docs/` - 文档更新
- `refactor/` - 代码重构
- `test/` - 测试相关
- `chore/` - 构建/工具相关

#### 3. 进行开发

```bash
# 安装依赖
cargo build

# 运行测试
cargo test

# 检查代码
cargo clippy
cargo fmt
```

#### 4. 提交更改

```bash
# 查看更改
git status
git diff

# 添加文件
git add path/to/file

# 提交
git commit -m "feat: add your feature description"
```

#### 5. 推送并创建 PR

```bash
# 推送到你的 fork
git push origin feature/your-feature-name

# 在 GitHub 上创建 Pull Request
```

---

## 🔄 开发流程

### 前置要求

- Rust 1.70 或更高版本
- Cargo（随 Rust 安装）
- Git

### 环境设置

```bash
# 克隆仓库
git clone https://github.com/your-org/bos.git
cd bos

# 安装开发依赖
cargo install cargo-watch cargo-nextest

# 启动 Zenoh（用于测试）
cargo install zenohd
zenohd
```

### 构建项目

```bash
# 开发构建
cargo build

# 发布构建
cargo build --release

# 检查代码（不构建）
cargo check

# 更新依赖
cargo update
```

### 运行测试

```bash
# 运行所有测试
cargo test

# 运行特定测试
cargo test test_name

# 运行特定包的测试
cargo test -p bus

# 运行集成测试
cargo test --test integration_test

# 显示测试输出
cargo test -- --nocapture

# 并行运行测试（更快）
cargo nextest run
```

### 运行示例

```bash
# 列出所有示例
cargo run --example

# 运行特定示例
cargo run --example example_name

# 运行 LLM 代理演示
cd examples/llm-agent-demo
cargo run --bin alice
```

---

## 📝 代码规范

### Rust 代码风格

我们使用标准 Rust 工具链来保持代码质量：

```bash
# 格式化代码
cargo fmt

# 检查代码风格
cargo fmt --check

# 运行 Clippy
cargo clippy

# 修复 Clippy 警告
cargo clippy --fix
```

### 代码组织

- 每个模块应该有清晰的职责
- 公共 API 需要完整的文档注释
- 使用 `#[allow(dead_code)]` 标记临时未使用的代码
- 遵循 Rust 命名约定：
  - 类型和 Traits：`PascalCase`
  - 函数和变量：`snake_case`
  - 常量：`SCREAMING_SNAKE_CASE`

### 文档注释

```rust
//! 模块级文档
//!
//! 详细描述模块的功能和用法。

/// 函数文档
///
/// # 参数
///
/// * `param1` - 参数描述
///
/// # 返回
///
/// 返回值描述
///
/// # 示例
///
/// ```
/// let result = function(arg);
/// assert_eq!(result, expected);
/// ```
pub fn function(param1: Type) -> ReturnType {
    // 实现
}
```

### 错误处理

- 使用 `Result<T, E>` 处理可恢复错误
- 使用 `anyhow::Result` 简化错误传播
- 为自定义错误类型实现 `std::error::Error`
- 提供有意义的错误消息

```rust
use anyhow::{Context, Result};

pub fn do_something() -> Result<()> {
    let value = get_value()
        .context("获取值失败")?;
    
    Ok(())
}
```

### 异步代码

- 使用 `#[tokio::test]` 进行异步测试
- 避免在热路径上阻塞
- 合理使用 `tokio::spawn` 和 `tokio::join!`

```rust
#[tokio::test]
async fn test_async_function() {
    let result = async_function().await;
    assert!(result.is_ok());
}
```

---

## 📦 提交规范

我们使用 [Conventional Commits](https://www.conventionalcommits.org/) 规范：

### 提交格式

```
<type>(<scope>): <subject>

<body>

<footer>
```

### Type 类型

- `feat`: 新功能
- `fix`: Bug 修复
- `docs`: 文档更新
- `style`: 代码格式（不影响功能）
- `refactor`: 重构（既不是新功能也不是修复）
- `perf`: 性能优化
- `test`: 添加测试
- `chore`: 构建过程或辅助工具的变动
- `ci`: CI 配置文件和脚本的变动
- `revert`: 回退之前的提交

### 示例

```bash
# 新功能
git commit -m "feat(agent): add tool registration support"

# Bug 修复
git commit -m "fix(rpc): resolve connection timeout issue"

# 文档更新
git commit -m "docs(readme): update installation instructions"

# 重构
git commit -m "refactor(bus): simplify session management"

# 破坏性变更
git commit -m "feat(api)!: change function signature

BREAKING CHANGE: function_name now requires additional parameter"
```

### Commitizen（可选）

安装 commitizen 工具辅助提交：

```bash
cargo install cz-cli
echo '{"path": "cz-conventional-changelog"}' > ~/.czrc

# 使用 cz 提交
git cz
```

---

## 🧪 测试

### 测试类型

1. **单元测试** - 测试单个函数或模块
2. **集成测试** - 测试多个组件的交互
3. **文档测试** - 在文档注释中嵌入测试
4. **性能测试** - 使用 criterion 进行基准测试

### 编写测试

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_functionality() {
        let input = "test";
        let result = process(input);
        assert_eq!(result, "expected");
    }

    #[tokio::test]
    async fn test_async_function() {
        let result = async_operation().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_error_handling() {
        let result = operation_that_fails();
        assert!(matches!(result, Err(_)));
    }
}
```

### 性能测试

```bash
# 运行性能测试
cargo bench

# 生成性能报告
cargo bench -- --output-format bencher
```

### 测试覆盖率

```bash
# 安装 tarpaulin
cargo install cargo-tarpaulin

# 生成覆盖率报告
cargo tarpaulin --out Html

# 查看报告
open tarpaulin-report.html
```

---

## 📚 文档

### API 文档

```bash
# 生成文档
cargo doc --no-deps

# 打开文档
cargo doc --open

# 生成所有版本的文档
cargo doc --all-features
```

### 文档要求

- 所有公共 API 必须有文档注释
- 包含使用示例
- 说明参数、返回值和可能的错误
- 标记不稳定的 API 为 `#[unstable]`

### 示例代码

示例应该：
- 可以直接运行
- 展示最佳实践
- 包含必要的注释
- 处理错误情况

```rust
/// 示例函数
///
/// # 示例
///
/// ```
/// use bos::prelude::*;
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let agent = Agent::new(config)?;
///     agent.run().await?;
///     Ok(())
/// }
/// ```
pub fn example() -> Result<()> {
    // 实现
}
```

---

## 🎯 Pull Request 检查清单

提交 PR 前，请确保：

- [ ] 代码通过 `cargo check`
- [ ] 代码通过 `cargo clippy`
- [ ] 代码通过 `cargo fmt --check`
- [ ] 所有测试通过 `cargo test`
- [ ] 添加了必要的测试
- [ ] 更新了相关文档
- [ ] 提交信息符合规范
- [ ] PR 描述清晰完整
- [ ] 没有引入不必要的依赖
- [ ] 没有破坏现有功能

---

## 📧 联系方式

- GitHub Issues: [问题反馈](https://github.com/your-org/bos/issues)
- GitHub Discussions: [讨论区](https://github.com/your-org/bos/discussions)
- Email: [your-email@example.com](mailto:your-email@example.com)

---

## 📄 许可证

通过贡献代码，您同意您的贡献将根据项目的 [MIT 许可证](LICENSE) 进行许可。

---

感谢您的贡献！🙏