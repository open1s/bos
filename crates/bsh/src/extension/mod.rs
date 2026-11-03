//! The BSH extension platform.
//!
//! This module is the runtime-independent extension system specified by
//! docs/bsh-extension.md:
//!
//! - a versioned manifest and package format;
//! - a managed store with hashing, staging, atomic install and a lockfile;
//! - a capability policy with explicit grants, scopes and an audit trail;
//! - a lifecycle state machine;
//! - seven runtimes - built-in, declarative, process, MCP, Extism, Wasm and Cordis;
//! - a bounded event bus;
//! - agent tools, commands, panels, prompts and contexts;
//! - a versioned declarative UI contract;
//! - diagnostics and a developer CLI.
//!
//! Nothing in this module imports GPUI. The UI is a consumer of the declarative
//! tree and the status rows, never a dependency of the platform.

// The platform is deliberately complete before every consumer lands: the
// storage namespace, the subscription side of the event bus and the UI action
// protocol are implemented and unit-tested here, while the first host build
// consumes tools, prompts, commands and status. The allow stays scoped to this
// module; code outside it still has to be wired to be accepted.
#![allow(dead_code)]

mod agent;
use std::sync::Arc;

mod cli;
#[cfg(test)]
mod e2e;
mod errors;
mod framework;
pub(crate) mod hooks;
pub(crate) mod host;
pub(crate) mod live;
pub(crate) mod notes;
pub(crate) mod terminal;
pub(crate) use host::shutdown;

/// The process-wide event dispatcher for extension tool seams.
static DISPATCHER: std::sync::OnceLock<Arc<ferris_sdk::composable::dispatch::EventDispatcher>> =
    std::sync::OnceLock::new();

pub(crate) fn dispatcher() -> Arc<ferris_sdk::composable::dispatch::EventDispatcher> {
    Arc::clone(
        DISPATCHER
            .get_or_init(|| Arc::new(ferris_sdk::composable::dispatch::EventDispatcher::new())),
    )
}

/// A process-wide Tokio runtime for extension work that starts outside a Tokio
/// context, such as a GPUI click listener. Extension runtimes own Tokio process
/// pipes and WASM async bindings, so spawning them from GPUI's executor alone
/// panics with "there is no reactor running".
///
/// The runtime is created once, on first use, and lives for the app.
fn extension_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("bsh-extension-runtime")
            .build()
            .expect("extension Tokio runtime")
    })
}

pub(crate) async fn run_on_tokio<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    extension_runtime().spawn(async move {
        let _ = tx.send(future.await);
    });
    rx.await.expect("extension runtime task")
}

/// Block a synchronous host callback on the extension runtime.
///
/// The call may already be inside a Tokio context, so the future always runs on
/// the dedicated multi-thread runtime rather than the caller's executor.
pub(crate) fn block_on_tokio_scoped<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    std::thread::scope(|scope| {
        scope
            .spawn(|| extension_runtime().block_on(future))
            .join()
            .expect("extension activation thread")
    })
}

pub(crate) fn block_on_tokio<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::spawn(move || extension_runtime().block_on(future))
            .join()
            .expect("extension activation thread")
    } else {
        extension_runtime().block_on(future)
    }
}

/// The managed extension root for a settings document.
pub(crate) fn store_root(settings: &crate::settings::Settings) -> std::path::PathBuf {
    host::extension_root(settings)
}
pub(crate) mod market;

pub(crate) use ferris_sdk::events::topics;
pub(crate) use host::{
    approve_process, cached_commands, cached_model_providers, cached_presets, grant_required,
    install_with_fibers, panels, publish, statuses, themes, uninstall, CommandSurface,
};

/// Run the developer CLI and return the process exit code.
pub(crate) fn cli(args: &[String]) -> i32 {
    cli::run(args)
}

#[cfg(test)]
mod bridge_tests {
    use super::run_on_tokio;

    #[tokio::test]
    async fn extension_work_runs_on_the_extension_runtime() {
        let output = run_on_tokio(async {
            tokio::process::Command::new("python3")
                .args(["-c", "print('bridge')"])
                .output()
                .await
        })
        .await
        .expect("process");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "bridge");
    }
}
