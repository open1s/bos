//! Starting the Runtime Host inside `bsh tui` (RFC-011, RFC-015).
//!
//! A terminal user should not have to start two processes to talk to one agent,
//! so `bsh tui` falls back to starting the Host itself when its data directory
//! has no owner yet. The interface is still a client: the Host is served on the
//! same local IPC endpoint an external `bsh host` would use, and the interface
//! reaches it through the Application Contract like any other client. A second
//! TUI, a GUI or a scripted client therefore attaches to the same sessions and
//! the same work instead of starting a competing authority.
//!
//! The lock, not a health probe, decides ownership. `RuntimeHost::open` refuses
//! a data directory another Host holds, and that refusal is exactly the signal
//! to attach instead of becoming a second authority. Because the OS releases
//! an advisory lock when its process ends, a killed `bsh tui` never leaves
//! behind an owner that blocks the next one.
//!
//! The trade-off of an embedded Host is process lifetime: a Host that runs
//! inside the client cannot outlive the client. Quitting drains it - in-flight
//! turns get the configured shutdown grace - and only then stops it. To keep
//! work running after the terminal closes, run `bsh host` first and every
//! `bsh tui` will attach to that one instead of starting another.

use std::time::{Duration, Instant};

use crate::runtime::config::RuntimeConfig;
use crate::runtime::contract::ShutdownMode;
use crate::runtime::endpoint::Endpoint;
use crate::runtime::host::{HostError, RuntimeHost};
use crate::runtime::server::{IpcServer, ServerError};

/// How long the endpoint may take to publish before startup counts as failed.
///
/// Binding the socket and writing the discovery record happen in the transport
/// task; a client that connected before the record exists would see "no Host"
/// for a Host that is already up.
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(5);

/// How long an attaching client waits for a Host that is still starting.
///
/// The owner publishes its endpoint right after it binds, so a `bsh tui` started
/// in the same breath as `bsh host` arrives before the record exists; calling
/// that Host missing would be a race, not a diagnosis.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(2);

/// How long pending transport work gets after the Host drained.
const STOP_TIMEOUT: Duration = Duration::from_millis(500);

/// Which Host serves the data directory of this process.
pub(crate) enum Authority {
    /// No Host owned the data directory, so this process started one and must
    /// stop it before it exits.
    Started(EmbeddedHost),
    /// Another Host already owns the data directory; this process only attaches.
    Attached,
}

impl Authority {
    /// Take the Host for `config`'s data directory: start one when it has no
    /// owner, attach when it has.
    ///
    /// Fails only when neither is possible: a Host that cannot start (broken
    /// configuration, unrecoverable journal, unusable provider) or a data
    /// directory another Host holds while that Host cannot be reached. The
    /// latter is reported by the client that follows, not swallowed here.
    pub(crate) fn open(config: RuntimeConfig) -> Result<Self, HostError> {
        match EmbeddedHost::start(config.clone()) {
            Ok(host) => Ok(Self::Started(host)),
            Err(HostError::AlreadyRunning { .. }) => {
                let deadline = Instant::now() + ATTACH_TIMEOUT;
                while Endpoint::discover(&config.data_dir).is_err() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(Self::Attached)
            }
            Err(error) => Err(error),
        }
    }
}

/// A Host that runs inside this process, served over local IPC.
pub(crate) struct EmbeddedHost {
    host: RuntimeHost,
    runtime: tokio::runtime::Runtime,
    server: tokio::task::JoinHandle<Result<(), ServerError>>,
}

impl EmbeddedHost {
    /// Open the Host for `config`, serve it, and return once a client could
    /// discover it.
    fn start(config: RuntimeConfig) -> Result<Self, HostError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                HostError::Config(format!("cannot start the async runtime: {error}"))
            })?;
        let host = runtime.block_on(RuntimeHost::open(config.clone()))?;
        let server = runtime.spawn(IpcServer::new(host.clone()).serve());
        wait_until_published(&host, &config, &server)?;
        log::info!(
            "embedded runtime host {} serving {} at {}",
            host.instance_id(),
            host.workspace_id(),
            config.socket_path.display()
        );
        Ok(Self {
            host,
            runtime,
            server,
        })
    }

    /// Identity of the Host this process runs.
    #[must_use]
    pub(crate) fn instance_id(&self) -> crate::runtime::ids::HostInstanceId {
        self.host.instance_id()
    }

    /// Drain the Host, stop serving and release the data directory.
    ///
    /// Idempotent with respect to a Host that is already draining: a client
    /// that saw `HostShutdown` and quit first, or another client that asked for
    /// the shutdown, finds the same drained state here.
    pub(crate) fn stop(self) {
        let Self {
            host,
            runtime,
            server,
        } = self;
        // Graceful first: a turn that is still producing a reply gets the
        // Host's configured grace before the process goes away.
        host.request_shutdown(ShutdownMode::Graceful);
        runtime.block_on(host.drain());
        // The transport ends when the phase leaves `Ready`; waiting for it is
        // what removes the socket and the discovery record, so the next
        // `bsh tui` starts a Host instead of dialling a dead endpoint.
        let _ = runtime.block_on(server);
        runtime.shutdown_timeout(STOP_TIMEOUT);
    }
}

/// Wait until the transport has published this Host's endpoint record.
fn wait_until_published(
    host: &RuntimeHost,
    config: &RuntimeConfig,
    server: &tokio::task::JoinHandle<Result<(), ServerError>>,
) -> Result<(), HostError> {
    let deadline = Instant::now() + PUBLISH_TIMEOUT;
    loop {
        // Matching the instance id, not just the file, keeps a record left by
        // a crashed Host from passing as this one's.
        if Endpoint::discover(&config.data_dir)
            .is_ok_and(|endpoint| endpoint.instance_id == host.instance_id())
        {
            return Ok(());
        }
        if server.is_finished() {
            return Err(HostError::Config(format!(
                "the local IPC transport stopped before it served {}",
                config.socket_path.display()
            )));
        }
        if Instant::now() >= deadline {
            return Err(HostError::Config(format!(
                "the local IPC transport did not publish {} within {}s",
                config.endpoint_path().display(),
                PUBLISH_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::runtime::client::RuntimeClient;
    use crate::runtime::contract::{CommandOutcome, ExecutionStatus};
    use crate::runtime::endpoint::EndpointError;

    fn config(dir: &Path) -> RuntimeConfig {
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let mut config = RuntimeConfig::with_data_dir(dir.join("runtime"));
        config.workspace_root = workspace;
        config.echo = true;
        // A test turn is already finished when the client stops the Host, and a
        // failing one must not wait out the production grace.
        config.shutdown_grace = Duration::ZERO;
        config
    }

    /// An embedded Host serves the same contract a detached one does, and stops
    /// only when the process that started it says so.
    #[test]
    fn an_embedded_host_serves_a_client_and_releases_the_directory_on_stop() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let data_dir = config.data_dir.clone();
        let Authority::Started(host) = Authority::open(config.clone()).unwrap() else {
            panic!("a fresh data directory must start a Host");
        };

        let (client, _frames) =
            RuntimeClient::connect(&data_dir, crate::runtime::client::tui_info()).unwrap();
        let session = client.create_session("embedded host").unwrap();
        let outcome = client.send_message(session, "hello in process").unwrap();
        let CommandOutcome::Accepted { .. } = outcome else {
            panic!("the embedded Host rejected the turn: {outcome:?}");
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let projection = client
                .session(session)
                .unwrap()
                .expect("the session exists");
            if projection
                .executions
                .iter()
                .all(|view| view.status == ExecutionStatus::Completed)
            {
                assert_eq!(projection.messages.len(), 2);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the embedded Host never completed the turn"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(host.instance_id(), client.instance_id());
        drop(client);

        host.stop();
        // The transport removed its record before stopping, so the next
        // `bsh tui` starts a Host instead of dialling a dead socket.
        assert!(matches!(
            Endpoint::discover(&data_dir),
            Err(EndpointError::Missing { .. })
        ));
        // The lock and the journal were released: recovery opens the same
        // directory again.
        let Authority::Started(again) = Authority::open(config).unwrap() else {
            panic!("the stopped Host must not keep the directory locked");
        };
        again.stop();
    }

    /// A second process is a client of the first Host, never a second authority.
    #[test]
    fn a_data_directory_with_a_host_attaches_instead_of_starting_another() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let data_dir = config.data_dir.clone();
        let Authority::Started(owner) = Authority::open(config.clone()).unwrap() else {
            panic!("the first open must start a Host");
        };
        assert!(matches!(
            Authority::open(config).unwrap(),
            Authority::Attached
        ));
        // Attaching left the owner alone: it is still the Host clients reach.
        let (client, _frames) =
            RuntimeClient::connect(&data_dir, crate::runtime::client::tui_info()).unwrap();
        assert_eq!(client.instance_id(), owner.instance_id());
        drop(client);
        owner.stop();
    }
}
