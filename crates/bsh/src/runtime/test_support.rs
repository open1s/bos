//! Shared harness for tests that need a real Host behind a real endpoint.
//!
//! The Host runs on its own thread with its own multi-thread runtime, exactly as
//! `bsh host` runs it, so a test connects over the same local IPC path a
//! terminal or a desktop client uses. Nothing here is reachable outside tests.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use crate::runtime::config::RuntimeConfig;
use crate::runtime::contract::HostPhase;
use crate::runtime::executor::TurnExecutor;
use crate::runtime::host::RuntimeHost;
use crate::runtime::server::IpcServer;

/// A serving Host plus the directory holding its endpoint record.
pub(crate) struct ServedHost {
    /// Kept alive so the Host's files outlive the test.
    dir: TempDir,
    /// Data directory a client passes to `RuntimeClient::connect`.
    data_dir: PathBuf,
    /// The Host thread; joining it proves the Host drained and released its
    /// endpoint.
    thread: std::thread::JoinHandle<()>,
}

impl ServedHost {
    /// Start a Host around `executor` and wait until it is listening.
    pub(crate) fn start(executor: Arc<dyn TurnExecutor>) -> Self {
        let dir = tempfile::tempdir().expect("test directory");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut config = RuntimeConfig::with_data_dir(dir.path().join("runtime"));
        config.workspace_root = workspace;
        config.shutdown_grace = Duration::ZERO;
        let data_dir = config.data_dir.clone();

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            runtime.block_on(async move {
                let host =
                    RuntimeHost::open_with_executor(config, executor, "test executor".to_string())
                        .await
                        .expect("the Host opens");
                let serving = tokio::spawn(IpcServer::new(host.clone()).serve());
                ready_tx.send(()).expect("the test is listening");
                // The client stops the Host when it shuts down; until it does,
                // the Host is the authority the test talks to.
                let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                while host.phase() == HostPhase::Ready {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "the test never stopped the Host"
                    );
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                host.drain().await;
                serving
                    .await
                    .expect("transport task")
                    .expect("transport served cleanly");
            });
        });
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the Host thread started");
        Self {
            dir,
            data_dir,
            thread,
        }
    }

    /// The directory a client connects to.
    pub(crate) fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// Wait for the Host to finish draining, then check it released its
    /// endpoint record.
    pub(crate) fn join(self) {
        self.thread.join().expect("server thread");
        assert!(
            !self.data_dir.join("runtime.json").exists(),
            "the discovery record outlived the Host"
        );
        // The endpoint record is written inside the data directory the
        // temporary directory owns; keeping `dir` until here is what makes the
        // assertion above meaningful.
        drop(self.dir);
    }
}
