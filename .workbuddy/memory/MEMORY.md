# BOS 项目长期备忘

## gpui-kit 测试宏遮蔽陷阱（bsh crate）

`use gpui_kit::*;` 在启用 test-support 时会带入 GPUI 的 `test` 宏，遮蔽 Rust 内建
`#[test]`。若该模块（或子模块经 `use super::*`）出现裸 `#[test]`，gpui::test 会
展开出 `#[test]` 再展开成 gpui::test，无限递归直到 recursion_limit（表现：先
SIGBUS 崩在 libgpui_macros 解析器，加 RUST_MIN_STACK 后报 recursion limit）。
规则（gpui-kit 官方注释亦如此要求）：任何含裸 `#[test]` 的模块不得 glob import
gpui_kit——用显式 import；测试模块用具体 `use super::{...}` 而非 `use super::*`。

## bsh 测试环境注意

- terminal/shell_tests 中派生真实交互 shell 的测试（约 6 个）在沙箱内必败：
  沙箱拦截 shell 启动期文件操作（如 `~/.zsh_history.LOCK`）。在父提交上同样失败，
  属环境限制非回归。agent crate 的 http_transport/seatbelt 测试同理（无网络）。
- bsh 测试套件约 160s 墙钟（RUST_TEST_THREADS=4 由 .cargo/config.toml 固定）。

## 扩展（codex/dsh）支持布局

- 解析/注册：crates/agent/src/extensions/（codex.rs、dsh.rs、declarative.rs、
  registry.rs、mod.rs）；测试在 crates/agent/tests/extensions.rs（18 个）。
- bsh 侧：market.rs（本地 Codex 缓存/DSH profile 发现 + 远程目录）、installed.rs
  （已安装列表视图 + (path,mtime) 缓存）、mcp.rs（扩展 MCP server 派生 + 信任门）、
  caps.rs（provider 贡献 → settings，refresh_plugin_providers）、
  settings/global.rs（InstalledExtension 记录）。fixture 在
  crates/agent/tests/fixtures/extensions/（codex-deploy、dsh-echo、dsh-provider）。
