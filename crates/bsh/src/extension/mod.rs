//! The BSH extension platform.
//!
//! This module is the runtime-independent extension system described in
//! docs/BSH_Extension_Framework_Design.md and specified by
//! docs/bsh-extension-standard.md:
//!
//! - a versioned manifest and package format;
//! - a managed store with hashing, staging, atomic install and a lockfile;
//! - a capability policy with explicit grants, scopes and an audit trail;
//! - a lifecycle state machine;
//! - three runtimes - built-in Rust, declarative files and external processes;
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
mod capability;
mod cli;
mod config;
#[cfg(test)]
mod e2e;
mod events;
pub(crate) mod host;
pub(crate) use host::shutdown;

/// A process-wide Tokio runtime for extension work that starts outside a Tokio
/// context, such as a GPUI click listener. Extension runtimes own Tokio process
/// pipes and WASM async bindings, so spawning them from GPUI's executor alone
/// panics with "there is no reactor running".
///
/// The runtime is created once, on first use, and lives for the app.
pub(crate) async fn run_on_tokio<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("bsh-extension-runtime")
            .build()
            .expect("extension Tokio runtime")
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    runtime.spawn(async move {
        let _ = tx.send(future.await);
    });
    rx.await.expect("extension runtime task")
}

/// The managed extension root for a settings document.
pub(crate) fn store_root(settings: &crate::settings::Settings) -> std::path::PathBuf {
    host::extension_root(settings)
}
mod lifecycle;
mod manifest;
pub(crate) mod market;
pub(crate) mod package;
mod resolver;
pub(crate) mod runtime;
mod signing;
mod sources;
mod storage;
pub(crate) mod types;
pub(crate) mod ui;

pub(crate) use events::topics;
pub(crate) use host::{
    approve_process, cached_commands, cached_model_providers, cached_presets, install, panels,
    publish, statuses, themes, CommandSurface,
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
