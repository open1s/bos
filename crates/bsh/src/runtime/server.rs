//! Local IPC transport for the Runtime Host (RFC-015).
//!
//! One Host serves one workspace over an owner-only endpoint: a Unix domain
//! socket on macOS/Linux, a named pipe on Windows. The adapter frames the
//! Application Contract and proves the caller's identity; it adds no domain
//! semantics of its own, so a second transport (HTTP/SSE, a future WebSocket)
//! can carry the same requests without touching the Host.
//!
//! # What the transport guarantees
//!
//! - The socket lives inside the Host's private (0700) data directory and is
//!   never created outside it.
//! - Every connection's principal is derived from OS peer credentials
//!   (`getpeereid` on macOS, `SO_PEERCRED` on Linux). A client cannot claim an
//!   identity in a frame: `Hello` carries registration data only.
//! - One request frame is bounded; a frame larger than the configured cap is
//!   refused instead of buffered.
//! - A slow client loses its *subscription* (with `SlowConsumer`) and never
//!   causes a committed event to be dropped: replay from the cursor is always
//!   available.
//! - Windows: the pipe is created with the process's default DACL, which
//!   admits the Host user, SYSTEM and Administrators. An administrator can read
//!   the data directory anyway, so the pipe is not a weaker path than the
//!   journal; the peer principal is the Host's local user.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Semaphore, broadcast, mpsc, watch};

use crate::runtime::authz::Principal;
use crate::runtime::config::RuntimeConfig;
use crate::runtime::contract::{
    ClientFrame, ClientInfo, ClientKind, EventEnvelope, Hello, HostPhase, PROTOCOL_VERSION,
    ProtocolError, ProtocolErrorCode, Request, Response, ResponseFrame, ServerFrame, Welcome,
};
use crate::runtime::endpoint::Endpoint;
use crate::runtime::host::{
    ActivityEvent, ConnectionContext, HostReply, LiveSubscription, RuntimeHost,
};

/// Failures that stop the transport.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// The endpoint cannot be bound.
    #[error("cannot bind {path}: {source}")]
    Bind {
        /// Path involved.
        path: String,
        /// OS error.
        #[source]
        source: std::io::Error,
    },
    /// The discovery record cannot be published.
    #[error("cannot publish {path}: {source}")]
    Endpoint {
        /// Path involved.
        path: String,
        /// OS error.
        #[source]
        source: std::io::Error,
    },
    /// The socket path is outside the Host's private data directory.
    #[error("socket path {0} is outside the Host data directory; refusing to serve it")]
    UnsafePath(String),
    /// The data directory itself is unusable.
    #[error("data directory {path}: {source}")]
    DataDir {
        /// Path involved.
        path: String,
        /// OS error.
        #[source]
        source: std::io::Error,
    },
}

/// How long a connection task gets to flush after the Host starts draining.
const DRAIN_FLUSH_GRACE: Duration = Duration::from_millis(200);

/// The local IPC listener.
pub struct IpcServer {
    host: RuntimeHost,
}

impl IpcServer {
    /// Build a listener for a Host. The endpoint paths come from its config.
    #[must_use]
    pub fn new(host: RuntimeHost) -> Self {
        Self { host }
    }

    /// Bind, publish the discovery record and serve until the Host stops.
    ///
    /// Returns only after the socket and the discovery record are removed and
    /// every connection task has finished, so a caller can treat the `Ok` as
    /// "no further client can reach this Host".
    pub async fn serve(self) -> Result<(), ServerError> {
        let config = self.host.config().clone();
        self.ensure_private_paths(&config)?;

        let mut listener = Listener::bind(&config).await?;
        publish_endpoint(&self.host, &config)?;
        let phase = self.host.watch_phase();
        let connections = Arc::new(Semaphore::new(config.max_connections));
        let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

        log::info!(
            "runtime host {} serving {} at {}",
            self.host.instance_id(),
            self.host.workspace_id(),
            config.socket_path.display()
        );

        loop {
            let permit = match connections.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => break,
            };
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                () = host_stopping(phase.clone()) => break,
            };
            let (connection, peer) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    log::warn!("runtime ipc accept failed: {error}");
                    continue;
                }
            };
            let principal = match peer {
                Ok(principal) => principal,
                Err(error) => {
                    // Fail closed: an unproven peer is never served.
                    log::warn!("runtime ipc connection refused: {error}");
                    drop(connection);
                    continue;
                }
            };
            let host = self.host.clone();
            let ctx = host.connection(principal);
            let max_frame = config.max_frame_bytes;
            let phase = self.host.watch_phase();
            tasks.push(tokio::spawn(async move {
                let _permit = permit;
                if let Err(error) = serve_connection(host, ctx, connection, max_frame, phase).await
                {
                    log::debug!("runtime ipc connection ended: {error}");
                }
            }));
        }

        // Draining: give every connection a bounded chance to flush its own
        // close frames, then abort the ones still reading.
        let deadline = tokio::time::Instant::now() + DRAIN_FLUSH_GRACE;
        loop {
            if tasks.iter().all(|task| task.is_finished())
                || tokio::time::Instant::now() >= deadline
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }

        remove_endpoint(self.host.instance_id(), &config);
        listener.cleanup().await;
        Ok(())
    }

    fn ensure_private_paths(&self, config: &RuntimeConfig) -> Result<(), ServerError> {
        std::fs::create_dir_all(&config.data_dir).map_err(|source| ServerError::DataDir {
            path: config.data_dir.display().to_string(),
            source,
        })?;
        if !config.socket_is_private() {
            return Err(ServerError::UnsafePath(
                config.socket_path.display().to_string(),
            ));
        }
        Ok(())
    }
}

/// One connection: handshake, then request/response plus live subscriptions.
async fn serve_connection<C>(
    host: RuntimeHost,
    ctx: ConnectionContext,
    connection: C,
    max_frame: usize,
    phase: watch::Receiver<HostPhase>,
) -> Result<(), String>
where
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(connection);
    let mut frames = FrameReader::default();

    // --- handshake ---------------------------------------------------------
    let first = frames
        .next(&mut reader, max_frame)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "connection closed before Hello".to_string())?;
    let ClientFrame {
        request_id,
        request,
    } = decode_client_frame(&first)?;
    let Request::Hello(hello) = request else {
        let response = Response::Error(ProtocolError {
            code: ProtocolErrorCode::BadRequest,
            message: "first frame must be a Hello".to_string(),
        });
        let _ = write_frame(
            &mut reader,
            &ServerFrame::Response(ResponseFrame {
                request_id,
                response,
            }),
        )
        .await;
        return Err("handshake refused: first frame was not Hello".to_string());
    };
    if hello.protocol_version != PROTOCOL_VERSION {
        let response = Response::Error(ProtocolError {
            code: ProtocolErrorCode::ProtocolVersion,
            message: format!(
                "client speaks protocol {}, this Host speaks {PROTOCOL_VERSION}",
                hello.protocol_version
            ),
        });
        let _ = write_frame(
            &mut reader,
            &ServerFrame::Response(ResponseFrame {
                request_id,
                response,
            }),
        )
        .await;
        return Err("handshake refused: protocol version".to_string());
    }
    let welcome = Welcome {
        protocol_version: PROTOCOL_VERSION,
        instance_id: host.instance_id(),
        client_instance_id: ctx.client_instance_id,
        principal: ctx.principal.view.clone(),
        workspace_id: host.workspace_id(),
        capabilities: capabilities(),
    };
    write_frame(
        &mut reader,
        &ServerFrame::Response(ResponseFrame {
            request_id,
            response: Response::Welcome(welcome),
        }),
    )
    .await
    .map_err(|error| error.to_string())?;
    log::debug!(
        "runtime client {} connected as {} ({})",
        hello.client.name,
        ctx.principal.view.display,
        client_kind(hello.client.kind)
    );

    // --- request/response --------------------------------------------------
    let inner = reader.into_inner();
    let (read_half, write_half) = tokio::io::split(inner);
    let mut reader = BufReader::new(read_half);
    let (outbound, mut outgoing) = mpsc::channel::<ServerFrame>(32);
    let writer = tokio::spawn(async move {
        let mut write_half = write_half;
        while let Some(frame) = outgoing.recv().await {
            if let Err(error) = write_frame(&mut write_half, &frame).await {
                log::warn!("runtime server writer failed: {error}");
                break;
            }
            log::debug!("runtime server wrote a frame");
        }
        let _ = write_half.flush().await;
    });

    let mut subscriptions: Vec<(u64, tokio::task::JoinHandle<()>)> = Vec::new();
    let mut budget = Budget::new(host.config().requests_per_second);
    loop {
        let next = tokio::select! {
            frame = frames.next(&mut reader, max_frame) => frame,
            () = host_stopping(phase.clone()) => {
                log::debug!("runtime server: the Host is stopping");
                break;
            }
        };
        let Some(bytes) = (match next {
            Ok(frame) => frame,
            Err(error) => {
                log::warn!("runtime server read failed: {error}");
                let _ = outbound
                    .send(ServerFrame::Response(ResponseFrame {
                        request_id: 0,
                        response: Response::Error(ProtocolError {
                            code: error.code(),
                            message: error.to_string(),
                        }),
                    }))
                    .await;
                break;
            }
        }) else {
            log::debug!("runtime server: the client closed the connection");
            break;
        };
        // A malformed frame still needs its correlation id echoed when one can
        // be recovered, so the client can fail that request instead of waiting.
        let request_id = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|value| value.get("request_id").and_then(|id| id.as_u64()))
            .unwrap_or(0);
        let ClientFrame {
            request_id,
            request,
        } = match decode_client_frame(&bytes) {
            Ok(frame) => frame,
            Err(message) => {
                if outbound
                    .send(ServerFrame::Response(ResponseFrame {
                        request_id,
                        response: Response::Error(ProtocolError {
                            code: ProtocolErrorCode::BadRequest,
                            message,
                        }),
                    }))
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
        };
        if !budget.allow() {
            let _ = outbound
                .send(ServerFrame::Response(ResponseFrame {
                    request_id,
                    response: Response::Error(ProtocolError {
                        code: ProtocolErrorCode::RateLimited,
                        message: "request budget exhausted for this connection".to_string(),
                    }),
                }))
                .await;
            continue;
        }
        let HostReply {
            response,
            subscription,
        } = host.handle_request(&ctx, request);
        log::debug!("runtime server handled request {request_id}");
        if outbound
            .send(ServerFrame::Response(ResponseFrame {
                request_id,
                response,
            }))
            .await
            .is_err()
        {
            break;
        }
        if let Some(subscription) = subscription {
            let LiveSubscription {
                id,
                receiver,
                activity,
            } = subscription;
            let outbound = outbound.clone();
            let host = host.clone();
            subscriptions.push((
                id,
                tokio::spawn(forward_subscription(host, outbound, id, receiver, activity)),
            ));
            subscriptions.retain(|(_, task)| !task.is_finished());
        }
    }

    // --- teardown ----------------------------------------------------------
    for (id, task) in subscriptions {
        task.abort();
        host.handle_request(
            &ctx,
            Request::Unsubscribe {
                subscription_id: id,
            },
        );
    }
    drop(outbound);
    let _ = writer.await;
    Ok(())
}

/// Forward one subscription's committed events and live activity to its
/// connection writer.
///
/// The two streams have different failure stories. A committed event the
/// journal dropped is recovered by replay from the client's cursor; a live
/// activity frame that overflowed only costs the client its partial text, so it
/// is told to reset instead of being handed a gap.
async fn forward_subscription(
    host: RuntimeHost,
    outbound: mpsc::Sender<ServerFrame>,
    subscription_id: u64,
    mut receiver: mpsc::Receiver<EventEnvelope>,
    mut activity: broadcast::Receiver<ActivityEvent>,
) {
    // Cleared when every sender is gone: the Host no longer runs turns, so a
    // closed activity channel must not spin the select loop. Committed events
    // still arrive until the journal closes its subscriber.
    let mut activity_open = true;
    let reason = loop {
        tokio::select! {
            event = receiver.recv() => match event {
                Some(event) => {
                    if outbound
                        .send(ServerFrame::Event {
                            subscription_id,
                            event: Box::new(event),
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                // The journal only drops a subscriber on overflow or when the
                // Host closes every subscriber while draining.
                None => {
                    break if host.phase() == HostPhase::Ready {
                        crate::runtime::contract::SubscriptionClosedReason::SlowConsumer
                    } else {
                        crate::runtime::contract::SubscriptionClosedReason::HostShutdown
                    };
                }
            },
            live = activity.recv(), if activity_open => match live {
                Ok(ActivityEvent {
                    session_id,
                    execution_id,
                    activity,
                }) => {
                    if outbound
                        .send(ServerFrame::Activity {
                            subscription_id,
                            session_id,
                            execution_id,
                            activity,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                // This connection's writer could not keep up with the deltas.
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    log::debug!(
                        "subscription {subscription_id} missed {missed} live activity frames"
                    );
                    if outbound
                        .send(ServerFrame::ActivityReset { subscription_id })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => activity_open = false,
            },
        }
    };
    let _ = outbound
        .send(ServerFrame::SubscriptionClosed {
            subscription_id,
            reason,
        })
        .await;
}

/// Resolve when the Host stops accepting new work.
async fn host_stopping(mut phase: watch::Receiver<HostPhase>) {
    loop {
        let current = *phase.borrow_and_update();
        if matches!(
            current,
            HostPhase::Draining | HostPhase::Stopped | HostPhase::Failed
        ) {
            return;
        }
        if phase.changed().await.is_err() {
            return;
        }
    }
}

fn capabilities() -> Vec<String> {
    [
        "sessions",
        "queries",
        "subscriptions",
        "approvals",
        "host-control",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn client_kind(kind: ClientKind) -> &'static str {
    match kind {
        ClientKind::Gui => "gui",
        ClientKind::Tui => "tui",
        ClientKind::Cli => "cli",
        ClientKind::Web => "web",
        ClientKind::Mobile => "mobile",
    }
}

fn decode_client_frame(bytes: &[u8]) -> Result<ClientFrame, String> {
    serde_json::from_slice::<ClientFrame>(bytes)
        .map_err(|error| format!("malformed request frame: {error}"))
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &ServerFrame,
) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(frame)
        .map_err(|error| std::io::Error::other(format!("frame cannot be encoded: {error}")))?;
    writer.write_all(&bytes).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

/// A per-connection request budget over a one-second window.
struct Budget {
    per_second: u32,
    window_start: tokio::time::Instant,
    count: u32,
}

impl Budget {
    fn new(per_second: u32) -> Self {
        Self {
            per_second: per_second.max(1),
            window_start: tokio::time::Instant::now(),
            count: 0,
        }
    }

    fn allow(&mut self) -> bool {
        let now = tokio::time::Instant::now();
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            self.window_start = now;
            self.count = 0;
        }
        self.count += 1;
        self.count <= self.per_second
    }
}

/// Why one frame could not be read.
#[derive(Debug, thiserror::Error)]
enum FrameError {
    #[error("frame exceeds the {limit}-byte limit")]
    TooLarge { limit: usize },
    #[error("frame is not newline-terminated")]
    Unterminated,
    #[error("frame read failed: {0}")]
    Io(#[from] std::io::Error),
}

impl FrameError {
    fn code(&self) -> ProtocolErrorCode {
        match self {
            Self::TooLarge { .. } => ProtocolErrorCode::FrameTooLarge,
            Self::Unterminated | Self::Io(_) => ProtocolErrorCode::BadRequest,
        }
    }
}

/// Newline-delimited frame reader.
///
/// The scratch buffer lives in the reader, so a cancelled read resumes on the
/// next call with the bytes it already consumed: `select!` against a Host
/// shutdown can never split a frame.
#[derive(Default)]
struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    async fn next<R: AsyncBufRead + Unpin>(
        &mut self,
        reader: &mut R,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, FrameError> {
        loop {
            // Everything between `fill_buf` returning and `consume` is
            // synchronous, so a cancelled read leaves `buf` intact.
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                return Err(FrameError::Unterminated);
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let take = newline.unwrap_or(available.len());
            if self.buf.len() + take > limit {
                return Err(FrameError::TooLarge { limit });
            }
            self.buf.extend_from_slice(&available[..take]);
            match newline {
                Some(index) => {
                    reader.consume(index + 1);
                    let frame = std::mem::take(&mut self.buf);
                    return Ok(Some(frame));
                }
                None => reader.consume(take),
            }
        }
    }
}

/// One accepted connection plus its derived principal.
type Accepted = (Connection, Result<Principal, String>);

#[cfg(unix)]
type Connection = tokio::net::UnixStream;

#[cfg(unix)]
struct Listener {
    inner: tokio::net::UnixListener,
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl Listener {
    async fn bind(config: &RuntimeConfig) -> Result<Self, ServerError> {
        let path = config.socket_path.clone();
        // We hold the Host instance lock, so a file at this path cannot belong
        // to a live Host; it is a leftover from a crashed process.
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ServerError::Bind {
                    path: path.display().to_string(),
                    source,
                });
            }
        }
        let inner = tokio::net::UnixListener::bind(&path).map_err(|source| ServerError::Bind {
            path: path.display().to_string(),
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(Self { inner, path })
    }

    async fn accept(&mut self) -> std::io::Result<Accepted> {
        let (stream, _addr) = self.inner.accept().await?;
        let principal = peer_principal(&stream);
        Ok((stream, principal))
    }

    async fn cleanup(self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn peer_principal(stream: &tokio::net::UnixStream) -> Result<Principal, String> {
    peer_uid(stream)
        .map(Principal::local)
        .map_err(|error| format!("peer credentials unavailable: {error}"))
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;
    // SAFETY: `ucred` is a plain C struct; the length is the struct's size.
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(credentials).cast(),
            &mut length,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(credentials.uid)
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
fn peer_uid(stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: both pointers are valid for the duration of the call.
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(uid)
}

#[cfg(all(
    unix,
    not(target_os = "linux"),
    not(target_os = "macos"),
    not(target_os = "ios"),
    not(target_os = "freebsd"),
    not(target_os = "openbsd"),
    not(target_os = "netbsd")
))]
fn peer_uid(_stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
    Err(std::io::Error::other(
        "this platform cannot prove a local peer's user id",
    ))
}

#[cfg(windows)]
type Connection = tokio::net::windows::named_pipe::NamedPipeServer;

#[cfg(windows)]
struct Listener {
    first: Connection,
    path: std::path::PathBuf,
}

#[cfg(windows)]
impl Listener {
    async fn bind(config: &RuntimeConfig) -> Result<Self, ServerError> {
        let path = config.socket_path.clone();
        // `first_pipe_instance` makes a second Host on the same name fail
        // instead of silently queueing behind this one.
        let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
        options.first_pipe_instance(true);
        let first = options.create(&path).map_err(|source| ServerError::Bind {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Self { first, path })
    }

    async fn accept(&mut self) -> std::io::Result<Accepted> {
        // Wait for the client this instance was created for, then create the
        // instance the NEXT client will connect to before serving this one.
        if let Err(error) = self.first.connect().await {
            let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
            options.first_pipe_instance(false);
            self.first = options.create(&self.path)?;
            return Err(error);
        }
        let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
        options.first_pipe_instance(false);
        let connection = std::mem::replace(&mut self.first, options.create(&self.path)?);
        // The pipe's default DACL admits only the Host user, SYSTEM and
        // Administrators; see the module note on Windows.
        Ok((
            connection,
            Ok(Principal::local(crate::runtime::authz::current_uid())),
        ))
    }

    async fn cleanup(self) {}
}

fn publish_endpoint(host: &RuntimeHost, config: &RuntimeConfig) -> Result<(), ServerError> {
    let endpoint = Endpoint {
        instance_id: host.instance_id(),
        workspace_id: host.workspace_id(),
        socket_path: config.socket_path.clone(),
        pid: std::process::id(),
        protocol_version: PROTOCOL_VERSION,
        started_at: host.status().started_at,
    };
    endpoint
        .write(&config.endpoint_path())
        .map_err(|error| ServerError::Endpoint {
            path: config.endpoint_path().display().to_string(),
            source: std::io::Error::other(error.to_string()),
        })
}

fn remove_endpoint(instance_id: crate::runtime::ids::HostInstanceId, config: &RuntimeConfig) {
    let path = config.endpoint_path();
    // Only our own record is removed: a newer Host that took the lock after a
    // crash may already have published its own.
    if let Ok(current) = Endpoint::read(&path)
        && current.instance_id == instance_id
    {
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod tests;

/// A client's registration payload, as the TUI sends it.
#[must_use]
pub fn tui_client_info() -> ClientInfo {
    ClientInfo {
        name: "bsh-tui".to_string(),
        kind: ClientKind::Tui,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// A client's registration payload for a scripted CLI.
#[must_use]
pub fn cli_client_info() -> ClientInfo {
    ClientInfo {
        name: "bsh-cli".to_string(),
        kind: ClientKind::Cli,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Build the `Hello` a client sends first.
#[must_use]
pub fn hello(info: ClientInfo) -> Hello {
    Hello {
        protocol_version: PROTOCOL_VERSION,
        client: info,
    }
}

/// Where a client should look for a Host, for diagnostics.
#[must_use]
pub fn endpoint_hint(data_dir: &Path) -> String {
    data_dir.join("runtime.json").display().to_string()
}
