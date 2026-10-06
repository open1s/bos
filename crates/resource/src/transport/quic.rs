//! QUIC transport (remote resources) built on `quinn` + `rustls`.
//!
//! - One connection per peer, multiplexed with bidirectional streams.
//! - `invoke`/`list`/`resolve` use request/response streams.
//! - `subscribe` uses a long-lived stream the server pumps events into.
//! - Peer certificate CN is the authenticated `agent` identity (mTLS).

use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use super::{Dispatcher, Transport};
use crate::action::{
    decode_action, decode_event, decode_output, encode_action, encode_event, encode_output,
    ResourceAction, ResourceEvent, ResourceOutput,
};
use crate::error::{ResourceError, Result};
use crate::meta::{ResourceInfo, ResourceType};
use log::debug;

// Frame tags.
const TAG_INVOKE: u8 = 1;

/// rustls 0.23 requires a crypto provider to be installed explicitly. Safe to
/// call more than once (subsequent calls are ignored).
fn ensure_crypto_provider() {
    let _ =
        rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider());
}

/// Responses larger than this are streamed as raw bytes on the QUIC stream
/// after a `Streaming` header frame, instead of being embedded in the frame.
const STREAM_THRESHOLD: usize = 64 * 1024;

/// Upper bound on a single frame's declared length. Guards against a malformed
/// or hostile `len` field triggering a multi-gigabyte allocation.
const MAX_FRAME: usize = 64 * 1024 * 1024;

/// If `out` is a data-bearing output (ReadOk/RecvOk) and its body is above the
/// streaming threshold, return `(kind, bytes)` so the caller can stream it.
fn streaming_payload(out: &ResourceOutput) -> Option<(u8, &Vec<u8>)> {
    match out {
        ResourceOutput::ReadOk { data } => Some((1, data)),
        ResourceOutput::RecvOk { data } => Some((2, data)),
        _ => None,
    }
}

/// Reconstruct the final output from a streamed `(kind, bytes)` pair.
fn from_streaming(kind: u8, data: Vec<u8>) -> ResourceOutput {
    match kind {
        2 => ResourceOutput::RecvOk { data },
        _ => ResourceOutput::ReadOk { data },
    }
}

const TAG_INVOKE_RESP: u8 = 1;
const TAG_SUB: u8 = 2;
const TAG_SUB_ACK: u8 = 2;
const TAG_EVENT: u8 = 3;
const TAG_LIST: u8 = 4;
const TAG_LIST_RESP: u8 = 4;
const TAG_RESOLVE: u8 = 5;
const TAG_RESOLVE_RESP: u8 = 5;
/// Error response frame: the body is the UTF-8 display of a `ResourceError`.
/// Sent instead of a result frame so the client receives a real error rather
/// than an abrupt end-of-stream ("early eof").
const TAG_ERR: u8 = 6;
// Data-plane streaming.
const TAG_STREAM_READ: u8 = 7;
const TAG_STREAM_WRITE: u8 = 8;
/// Stream header: payload = u64 LE total length hint (`u64::MAX` = unknown).
const TAG_STREAM_HDR: u8 = 9;
const TAG_CHUNK: u8 = 10;
const TAG_STREAM_END: u8 = 11;
/// Length hint sent when the sender doesn't know the total up front.
const LEN_UNKNOWN: u64 = u64::MAX;

fn kind_byte(kind: Option<ResourceType>) -> u8 {
    match kind {
        None => 0,
        Some(ResourceType::Storage) => 1,
        Some(ResourceType::Network) => 2,
        Some(ResourceType::Compute) => 3,
        Some(ResourceType::System) => 4,
        Some(ResourceType::Abstract) => 5,
        Some(ResourceType::Combine) => 6,
    }
}

fn byte_kind(b: u8) -> Option<ResourceType> {
    match b {
        0 => None,
        1 => Some(ResourceType::Storage),
        2 => Some(ResourceType::Network),
        3 => Some(ResourceType::Compute),
        4 => Some(ResourceType::System),
        5 => Some(ResourceType::Abstract),
        6 => Some(ResourceType::Combine),
        _ => None,
    }
}

async fn write_frame<S>(stream: &mut S, tag: u8, payload: &[u8]) -> Result<()>
where
    S: AsyncWriteExt + Unpin,
{
    let mut buf = Vec::with_capacity(payload.len() + 5);
    buf.push(tag);
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    stream
        .write_all(&buf)
        .await
        .map_err(|e| ResourceError::Transport(e.to_string()))
}

async fn read_frame<S>(stream: &mut S) -> Result<(u8, Vec<u8>)>
where
    S: AsyncReadExt + Unpin,
{
    let mut tag = [0u8; 1];
    stream
        .read_exact(&mut tag)
        .await
        .map_err(|e| ResourceError::Transport(e.to_string()))?;
    let mut len = [0u8; 4];
    stream
        .read_exact(&mut len)
        .await
        .map_err(|e| ResourceError::Transport(e.to_string()))?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(ResourceError::Transport(format!("frame too large: {len}")));
    }
    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(|e| ResourceError::Transport(e.to_string()))?;
    Ok((tag[0], payload))
}

// ---------- client ----------

/// QUIC client transport.
pub struct QuicTransport {
    endpoint: Endpoint,
    client_config: ClientConfig,
    server_addr: SocketAddr,
    server_name: String,
    conn: Mutex<Option<Connection>>,
}

impl QuicTransport {
    /// Build a client connecting to `server_addr` (SNI `server_name`), trusting
    /// `trust_pem` and presenting `client_cert_der`/`client_key_der`.
    pub fn new(
        server_addr: SocketAddr,
        server_name: String,
        trust_pem: &[u8],
        client_cert_der: CertificateDer<'static>,
        client_key_der: PrivateKeyDer<'static>,
    ) -> Result<Self> {
        ensure_crypto_provider();
        let mut roots = rustls::RootCertStore::empty();
        for c in rustls_pemfile::certs(&mut Cursor::new(trust_pem))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| ResourceError::Transport(e.to_string()))?
        {
            roots
                .add(c)
                .map_err(|e| ResourceError::Transport(e.to_string()))?;
        }
        let client_tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(vec![client_cert_der], client_key_der)
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        let quic_client = QuicClientConfig::try_from(client_tls)
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        let mut client_config = ClientConfig::new(Arc::new(quic_client));
        // Liveness on idle links: QUIC PINGs keep NAT mappings and server
        // idle timeouts from silently dropping long-lived connections.
        let mut transport_cfg = quinn::TransportConfig::default();
        transport_cfg.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
        client_config.transport_config(Arc::new(transport_cfg));
        let bind_addr: SocketAddr = "0.0.0.0:0"
            .parse()
            .map_err(|e: std::net::AddrParseError| ResourceError::Transport(e.to_string()))?;
        let endpoint =
            Endpoint::client(bind_addr).map_err(|e| ResourceError::Transport(e.to_string()))?;
        debug!(
            "QUIC client endpoint bound to {} -> {}",
            bind_addr, server_addr
        );
        Ok(Self {
            endpoint,
            client_config,
            server_addr,
            server_name,
            conn: Mutex::new(None),
        })
    }

    async fn connection(&self) -> Result<Connection> {
        let mut guard = self.conn.lock().await;
        if let Some(c) = guard.as_ref() {
            if c.close_reason().is_none() {
                debug!("QUIC connection reused for {}", self.server_addr);
                return Ok(c.clone());
            }
        }
        debug!(
            "QUIC establishing new connection to {} (server_name={})",
            self.server_addr, self.server_name
        );
        let connecting = self
            .endpoint
            .connect_with(
                self.client_config.clone(),
                self.server_addr,
                &self.server_name,
            )
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        let conn = connecting
            .await
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        debug!(
            "QUIC connection established: remote={}",
            conn.remote_address()
        );
        *guard = Some(conn.clone());
        Ok(conn)
    }
}

#[async_trait]
impl Transport for QuicTransport {
    async fn invoke(&self, uri: &str, action: ResourceAction) -> Result<ResourceOutput> {
        debug!("QUIC invoke: uri={} action={}", uri, action.name());
        let conn = self.connection().await?;
        let mut stream = conn
            .open_bi()
            .await
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        debug!("QUIC stream opened for invoke: uri={}", uri);
        let mut payload = Vec::new();
        payload.extend_from_slice(&(uri.len() as u32).to_le_bytes());
        payload.extend_from_slice(uri.as_bytes());
        payload.extend_from_slice(&encode_action(&action)?);
        write_frame(&mut stream.0, TAG_INVOKE, &payload).await?;
        let _ = stream.0.finish();

        let (tag, body) = read_frame(&mut stream.1).await?;
        if tag == TAG_ERR {
            // The server dispatched and returned a real error — surface it.
            return Err(ResourceError::Other(
                String::from_utf8_lossy(&body).into_owned(),
            ));
        }
        if tag != TAG_INVOKE_RESP {
            return Err(ResourceError::Transport(format!("unexpected tag {tag}")));
        }
        let out = decode_output(&body)?;
        debug!("QUIC invoke response: uri={} tag={}", uri, tag);
        // Large bodies arrive as raw bytes following the header frame.
        if let ResourceOutput::Streaming { kind, total } = out {
            let len = total.ok_or_else(|| {
                ResourceError::Transport("streaming response missing length".to_string())
            })? as usize;
            debug!(
                "QUIC streaming response: uri={} kind={} len={}",
                uri,
                kind,
                total.unwrap_or(0)
            );
            let mut buf = vec![0u8; len];
            stream
                .1
                .read_exact(&mut buf)
                .await
                .map_err(|e| ResourceError::Transport(e.to_string()))?;
            return Ok(from_streaming(kind, buf));
        }
        debug!("QUIC invoke completed: uri={}", uri);
        Ok(out)
    }

    async fn subscribe(
        &self,
        uri: &str,
        events: Vec<String>,
    ) -> Result<BoxStream<'static, ResourceEvent>> {
        debug!("QUIC subscribe: uri={} events={:?}", uri, events);
        let conn = self.connection().await?;
        let mut stream = conn
            .open_bi()
            .await
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        debug!("QUIC stream opened for subscribe: uri={}", uri);
        let mut payload = Vec::new();
        payload.extend_from_slice(&(uri.len() as u32).to_le_bytes());
        payload.extend_from_slice(uri.as_bytes());
        payload.extend_from_slice(&(events.len() as u32).to_le_bytes());
        for e in &events {
            payload.extend_from_slice(&(e.len() as u32).to_le_bytes());
            payload.extend_from_slice(e.as_bytes());
        }
        write_frame(&mut stream.0, TAG_SUB, &payload).await?;
        let (tag, ack) = read_frame(&mut stream.1).await?;
        if tag == TAG_ERR {
            return Err(ResourceError::Other(
                String::from_utf8_lossy(&ack).into_owned(),
            ));
        }
        debug!("QUIC subscribe ack received: uri={}", uri);
        let recv = stream.1;
        let out = async_stream::stream! {
            let mut recv = recv;
            while let Ok((TAG_EVENT, body)) = read_frame(&mut recv).await {
                match decode_event(&body) {
                    Ok(ev) => yield ev,
                    Err(_) => break,
                }
            }
        };
        debug!("QUIC subscribe stream ready: uri={}", uri);
        Ok(out.boxed())
    }

    async fn list(&self, _agent: &str, kind: Option<ResourceType>) -> Result<Vec<ResourceInfo>> {
        // The server derives the peer identity from the mTLS cert CN; the local
        // `agent` argument is not sent over the wire (the cert already proves it).
        debug!("QUIC list: kind={:?}", kind);
        let conn = self.connection().await?;
        let mut stream = conn
            .open_bi()
            .await
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        write_frame(&mut stream.0, TAG_LIST, &[kind_byte(kind)]).await?;
        let _ = stream.0.finish();
        let (tag, body) = read_frame(&mut stream.1).await?;
        if tag == TAG_ERR {
            return Err(ResourceError::Other(
                String::from_utf8_lossy(&body).into_owned(),
            ));
        }
        let infos: Vec<ResourceInfo> =
            serde_json::from_slice(&body).map_err(|e| ResourceError::Codec(e.to_string()))?;
        debug!("QUIC list returned {} resources", infos.len());
        Ok(infos)
    }

    async fn resolve(&self, _agent: &str, uri: &str) -> Result<Option<ResourceInfo>> {
        debug!("QUIC resolve: uri={}", uri);
        let conn = self.connection().await?;
        let mut stream = conn
            .open_bi()
            .await
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        let mut payload = Vec::new();
        payload.extend_from_slice(&(uri.len() as u32).to_le_bytes());
        payload.extend_from_slice(uri.as_bytes());
        write_frame(&mut stream.0, TAG_RESOLVE, &payload).await?;
        let _ = stream.0.finish();
        let (tag, body) = read_frame(&mut stream.1).await?;
        if tag == TAG_ERR {
            return Err(ResourceError::Other(
                String::from_utf8_lossy(&body).into_owned(),
            ));
        }
        if body.is_empty() {
            debug!("QUIC resolve not found: uri={}", uri);
            return Ok(None);
        }
        let info: ResourceInfo =
            serde_json::from_slice(&body[1..]).map_err(|e| ResourceError::Codec(e.to_string()))?;
        debug!("QUIC resolve found: uri={}", uri);
        Ok(Some(info))
    }

    async fn read_stream(
        &self,
        uri: &str,
        offset: u64,
        len: Option<u64>,
    ) -> Result<super::ChunkStream> {
        debug!(
            "QUIC read_stream: uri={} offset={} len={:?}",
            uri, offset, len
        );
        let conn = self.connection().await?;
        let mut stream = conn
            .open_bi()
            .await
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        let payload = encode_stream_read_req(uri, offset, len);
        write_frame(&mut stream.0, TAG_STREAM_READ, &payload).await?;
        let _ = stream.0.finish();
        // Response header: TAG_STREAM_HDR (length hint) or TAG_ERR.
        let (tag, body) = read_frame(&mut stream.1).await?;
        match tag {
            TAG_STREAM_HDR => {}
            TAG_ERR => {
                return Err(ResourceError::Other(
                    String::from_utf8_lossy(&body).into_owned(),
                ))
            }
            other => return Err(ResourceError::Transport(format!("unexpected tag {other}"))),
        }
        let mut recv = stream.1;
        let chunks = async_stream::stream! {
            loop {
                match read_frame(&mut recv).await {
                    Ok((TAG_CHUNK, data)) => yield Ok(data),
                    Ok((TAG_STREAM_END, _)) => break,
                    Ok((TAG_ERR, body)) => {
                        yield Err(String::from_utf8_lossy(&body).into_owned());
                        break;
                    }
                    Ok((other, _)) => {
                        yield Err(format!("unexpected stream tag {other}"));
                        break;
                    }
                    Err(e) => {
                        yield Err(e.to_string());
                        break;
                    }
                }
            }
        };
        Ok(Box::pin(chunks))
    }

    async fn write_stream(&self, uri: &str, offset: u64) -> Result<Box<dyn super::ChunkWriter>> {
        debug!("QUIC write_stream: uri={} offset={}", uri, offset);
        let conn = self.connection().await?;
        let mut stream = conn
            .open_bi()
            .await
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        let payload = encode_stream_write_req(uri, offset);
        write_frame(&mut stream.0, TAG_STREAM_WRITE, &payload).await?;
        // Server acks with TAG_STREAM_HDR once the handler is ready.
        let (tag, body) = read_frame(&mut stream.1).await?;
        match tag {
            TAG_STREAM_HDR => {}
            TAG_ERR => {
                return Err(ResourceError::Other(
                    String::from_utf8_lossy(&body).into_owned(),
                ))
            }
            other => return Err(ResourceError::Transport(format!("unexpected tag {other}"))),
        }
        Ok(Box::new(QuicChunkWriter {
            send: stream.0,
            recv: stream.1,
            written: 0,
        }))
    }
}

/// Client-side write half of a data-plane stream: chunks become `TAG_CHUNK`
/// frames; `finish` sends `TAG_STREAM_END` and awaits the server's `WriteOk`.
struct QuicChunkWriter {
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    written: u64,
}

#[async_trait]
impl super::ChunkWriter for QuicChunkWriter {
    async fn write_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        write_frame(&mut self.send, TAG_CHUNK, chunk).await?;
        self.written += chunk.len() as u64;
        Ok(())
    }

    async fn finish(&mut self) -> Result<u64> {
        write_frame(&mut self.send, TAG_STREAM_END, &[]).await?;
        let _ = self.send.finish();
        let (tag, body) = read_frame(&mut self.recv).await?;
        match tag {
            TAG_INVOKE_RESP => match decode_output(&body)? {
                ResourceOutput::WriteOk { written } => Ok(written),
                other => Err(ResourceError::Transport(format!(
                    "unexpected write ack {other:?}"
                ))),
            },
            TAG_ERR => Err(ResourceError::Other(
                String::from_utf8_lossy(&body).into_owned(),
            )),
            other => Err(ResourceError::Transport(format!("unexpected tag {other}"))),
        }
    }
}

fn encode_stream_read_req(uri: &str, offset: u64, len: Option<u64>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(uri.len() + 21);
    buf.extend_from_slice(&(uri.len() as u32).to_le_bytes());
    buf.extend_from_slice(uri.as_bytes());
    buf.extend_from_slice(&offset.to_le_bytes());
    buf.push(if len.is_some() { 1 } else { 0 });
    buf.extend_from_slice(&len.unwrap_or(0).to_le_bytes());
    buf
}

fn encode_stream_write_req(uri: &str, offset: u64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(uri.len() + 12);
    buf.extend_from_slice(&(uri.len() as u32).to_le_bytes());
    buf.extend_from_slice(uri.as_bytes());
    buf.extend_from_slice(&offset.to_le_bytes());
    buf
}

fn decode_offset_len(body: &[u8]) -> Result<(String, u64, Option<u64>)> {
    if body.len() < 4 {
        return Err(ResourceError::Transport("short stream-read req".into()));
    }
    let ulen = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
    if body.len() < 4 + ulen + 8 + 9 {
        return Err(ResourceError::Transport("short stream-read req".into()));
    }
    let uri = String::from_utf8_lossy(&body[4..4 + ulen]).to_string();
    let at = 4 + ulen;
    let offset = u64::from_le_bytes(body[at..at + 8].try_into().unwrap());
    let has_len = body[at + 8] != 0;
    let len = u64::from_le_bytes(body[at + 9..at + 17].try_into().unwrap());
    Ok((uri, offset, if has_len { Some(len) } else { None }))
}

fn decode_offset(body: &[u8]) -> Result<(String, u64)> {
    if body.len() < 4 {
        return Err(ResourceError::Transport("short stream-write req".into()));
    }
    let ulen = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
    if body.len() < 4 + ulen + 8 {
        return Err(ResourceError::Transport("short stream-write req".into()));
    }
    let uri = String::from_utf8_lossy(&body[4..4 + ulen]).to_string();
    let offset = u64::from_le_bytes(body[4 + ulen..4 + ulen + 8].try_into().unwrap());
    Ok((uri, offset))
}

// ---------- server ----------

/// QUIC server. Binds an endpoint and dispatches incoming streams to a
/// [`Dispatcher`], using the peer certificate CN as the agent identity.
pub struct QuicServer {
    dispatcher: Arc<dyn Dispatcher>,
    endpoint: Mutex<Option<Endpoint>>,
}

impl QuicServer {
    /// Create a server around `dispatcher`.
    pub fn new(dispatcher: Arc<dyn Dispatcher>) -> Self {
        Self {
            dispatcher,
            endpoint: Mutex::new(None),
        }
    }

    /// Bind the server endpoint on `addr`, trusting `ca_pem` for client mTLS and
    /// presenting `cert_der`/`key_der`. Returns the actual bound `SocketAddr`.
    /// `extra_client_cas` adds additional CAs trusted for client verification
    /// (e.g. peer CAs from bus discovery).
    pub async fn bind(
        &self,
        addr: SocketAddr,
        cert_der: CertificateDer<'static>,
        key_der: PrivateKeyDer<'static>,
        ca_pem: &[u8],
        extra_client_cas: &[&[u8]],
    ) -> Result<SocketAddr> {
        ensure_crypto_provider();
        let mut roots = rustls::RootCertStore::empty();
        for c in parse_ca(ca_pem)? {
            roots
                .add(c)
                .map_err(|e| ResourceError::Transport(e.to_string()))?;
        }
        for extra_pem in extra_client_cas {
            for c in parse_ca(extra_pem)? {
                roots
                    .add(c)
                    .map_err(|e| ResourceError::Transport(e.to_string()))?;
            }
        }
        let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        let server_tls = rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(vec![cert_der], key_der)
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        let quic_server = QuicServerConfig::try_from(server_tls)
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        let config = ServerConfig::with_crypto(Arc::new(quic_server));
        let endpoint =
            Endpoint::server(config, addr).map_err(|e| ResourceError::Transport(e.to_string()))?;
        let local = endpoint
            .local_addr()
            .map_err(|e| ResourceError::Transport(e.to_string()))?;
        *self.endpoint.lock().await = Some(endpoint);
        Ok(local)
    }

    /// Run the accept loop. Must be called after [`QuicServer::bind`].
    pub async fn run(&self) -> Result<()> {
        let endpoint = self
            .endpoint
            .lock()
            .await
            .take()
            .ok_or_else(|| ResourceError::Transport("QUIC server not bound".to_string()))?;
        while let Some(conn) = endpoint.accept().await {
            let conn = match conn.await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("QUIC handshake failed: {e}");
                    continue;
                }
            };
            let agent = extract_cn(&conn).unwrap_or_else(|| "unknown".to_string());
            let disp = self.dispatcher.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_conn(disp, conn, agent).await {
                    tracing::debug!("connection ended: {e}");
                }
            });
        }
        Ok(())
    }
}

fn parse_ca(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>> {
    rustls_pemfile::certs(&mut Cursor::new(pem))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map(|c| c.into_iter().map(|c| c.into_owned()).collect())
        .map_err(|e| ResourceError::Transport(e.to_string()))
}

fn extract_cn(conn: &Connection) -> Option<String> {
    let data = conn.peer_identity()?;
    let certs = data.downcast_ref::<Vec<CertificateDer>>()?;
    let first = certs.first()?;
    let (_, parsed) = x509_parser::parse_x509_certificate(first.as_ref()).ok()?;
    for attr in parsed.subject().iter_common_name() {
        if let Ok(cn) = attr.as_str() {
            return Some(cn.to_string());
        }
    }
    None
}

async fn handle_conn(disp: Arc<dyn Dispatcher>, conn: Connection, agent: String) -> Result<()> {
    loop {
        let stream = match conn.accept_bi().await {
            Ok(s) => s,
            Err(quinn::ConnectionError::ApplicationClosed { .. }) => break,
            Err(e) => return Err(ResourceError::Transport(e.to_string())),
        };
        let disp = disp.clone();
        let agent = agent.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_stream(disp, stream, agent).await {
                tracing::debug!("stream ended: {e}");
            }
        });
    }
    Ok(())
}

async fn handle_stream(
    disp: Arc<dyn Dispatcher>,
    mut stream: (quinn::SendStream, quinn::RecvStream),
    agent: String,
) -> Result<()> {
    loop {
        let (tag, payload) = read_frame(&mut stream.1).await?;
        match tag {
            TAG_INVOKE => {
                let (uri, action) = match parse_uri_action(&payload) {
                    Ok(v) => v,
                    Err(e) => {
                        // Tell the client about the malformed frame instead of
                        // leaving it facing an unexplained end-of-stream.
                        write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await?;
                        continue;
                    }
                };
                let out = disp.dispatch(&agent, &uri, action).await;
                match out {
                    Ok(o) => {
                        // Stream large data bodies as raw bytes after a header frame.
                        if let Some((kind, data)) = streaming_payload(&o) {
                            if data.len() > STREAM_THRESHOLD {
                                let header = ResourceOutput::Streaming {
                                    kind,
                                    total: Some(data.len() as u64),
                                };
                                write_frame(
                                    &mut stream.0,
                                    TAG_INVOKE_RESP,
                                    &encode_output(&header)?,
                                )
                                .await?;
                                stream
                                    .0
                                    .write_all(data)
                                    .await
                                    .map_err(|e| ResourceError::Transport(e.to_string()))?;
                                return Ok(());
                            }
                        }
                        let body = encode_output(&o)?;
                        write_frame(&mut stream.0, TAG_INVOKE_RESP, &body).await?;
                    }
                    Err(e) => {
                        // Dispatch failures (NotFound, PolicyDenied, …) go back
                        // to the caller as a frames, never as a silent close.
                        write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await?;
                    }
                }
            }
            TAG_SUB => {
                let (uri, events) = match parse_uri_events(&payload) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await;
                        continue;
                    }
                };
                let sub = match disp.subscribe(&agent, &uri, events).await {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await;
                        continue;
                    }
                };
                write_frame(&mut stream.0, TAG_SUB_ACK, &[]).await?;
                let mut sub: BoxStream<ResourceEvent> = sub;
                while let Some(ev) = sub.next().await {
                    let body = encode_event(&ev)?;
                    write_frame(&mut stream.0, TAG_EVENT, &body).await?;
                }
            }
            TAG_LIST => {
                let kind = byte_kind(payload.first().copied().unwrap_or(0));
                let infos = match disp.list(&agent, kind).await {
                    Ok(i) => i,
                    Err(e) => {
                        let _ = write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await;
                        continue;
                    }
                };
                let body = match serde_json::to_vec(&infos) {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await;
                        continue;
                    }
                };
                write_frame(&mut stream.0, TAG_LIST_RESP, &body).await?;
            }
            TAG_RESOLVE => {
                if payload.len() < 4 {
                    let _ = write_frame(&mut stream.0, TAG_ERR, b"short resolve").await;
                    continue;
                }
                let uri = String::from_utf8_lossy(&payload[4..]).to_string();
                let info = match disp.resolve(&agent, &uri).await {
                    Ok(i) => i,
                    Err(e) => {
                        let _ = write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await;
                        continue;
                    }
                };
                let body = match info {
                    Some(i) => {
                        let mut b = vec![1u8];
                        let j = match serde_json::to_vec(&i) {
                            Ok(j) => j,
                            Err(e) => {
                                let _ =
                                    write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes())
                                        .await;
                                continue;
                            }
                        };
                        b.extend_from_slice(&j);
                        b
                    }
                    None => vec![0u8],
                };
                write_frame(&mut stream.0, TAG_RESOLVE_RESP, &body).await?;
            }
            TAG_STREAM_READ => {
                let (uri, offset, len) = match decode_offset_len(&payload) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await;
                        continue;
                    }
                };
                let chunks = match disp.read_stream(&agent, &uri, offset, len).await {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await;
                        continue;
                    }
                };
                // Length hint is unknown ahead of time (server never pre-stat's
                // or buffers); clients must treat the header as informational.
                write_frame(&mut stream.0, TAG_STREAM_HDR, &LEN_UNKNOWN.to_le_bytes()).await?;
                let mut chunks = chunks;
                loop {
                    match chunks.next().await {
                        Some(Ok(bytes)) => {
                            write_frame(&mut stream.0, TAG_CHUNK, &bytes).await?;
                        }
                        Some(Err(e)) => {
                            let _ = write_frame(&mut stream.0, TAG_ERR, e.as_bytes()).await;
                            break;
                        }
                        None => {
                            write_frame(&mut stream.0, TAG_STREAM_END, &[]).await?;
                            break;
                        }
                    }
                }
            }
            TAG_STREAM_WRITE => {
                let (uri, offset) = match decode_offset(&payload) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await;
                        continue;
                    }
                };
                let mut writer = match disp.write_stream(&agent, &uri, offset).await {
                    Ok(w) => w,
                    Err(e) => {
                        let _ = write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes()).await;
                        continue;
                    }
                };
                write_frame(&mut stream.0, TAG_STREAM_HDR, &LEN_UNKNOWN.to_le_bytes()).await?;
                // Consume chunks until END or client disconnect; the final
                // WriteOk/ERR frame is the completion signal.
                loop {
                    match read_frame(&mut stream.1).await {
                        Ok((TAG_CHUNK, data)) => {
                            if let Err(e) = writer.write_chunk(&data).await {
                                let _ =
                                    write_frame(&mut stream.0, TAG_ERR, e.to_string().as_bytes())
                                        .await;
                                break;
                            }
                        }
                        Ok((TAG_STREAM_END, _)) => {
                            match writer.finish().await {
                                Ok(written) => {
                                    let out = ResourceOutput::WriteOk { written };
                                    let body = encode_output(&out)?;
                                    write_frame(&mut stream.0, TAG_INVOKE_RESP, &body).await?;
                                }
                                Err(e) => {
                                    let _ = write_frame(
                                        &mut stream.0,
                                        TAG_ERR,
                                        e.to_string().as_bytes(),
                                    )
                                    .await;
                                }
                            }
                            break;
                        }
                        Ok((TAG_ERR, _)) => break,
                        Ok((other, _)) => {
                            let _ = write_frame(
                                &mut stream.0,
                                TAG_ERR,
                                format!("unexpected stream tag {other}").as_bytes(),
                            )
                            .await;
                            break;
                        }
                        Err(_) => break, // client went away mid-transfer
                    }
                }
            }
            other => return Err(ResourceError::Transport(format!("bad tag {other}"))),
        }
    }
}

fn parse_uri_action(payload: &[u8]) -> Result<(String, ResourceAction)> {
    if payload.len() < 4 {
        return Err(ResourceError::Transport("short invoke".into()));
    }
    let ulen = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
    if ulen > payload.len() - 4 {
        return Err(ResourceError::Transport("bad invoke uri length".into()));
    }
    let uri = String::from_utf8_lossy(&payload[4..4 + ulen]).to_string();
    let action = decode_action(&payload[4 + ulen..])?;
    Ok((uri, action))
}

fn parse_uri_events(payload: &[u8]) -> Result<(String, Vec<String>)> {
    if payload.len() < 4 {
        return Err(ResourceError::Transport("short sub".into()));
    }
    let ulen = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
    if ulen > payload.len() - 4 {
        return Err(ResourceError::Transport("bad sub uri length".into()));
    }
    let uri = String::from_utf8_lossy(&payload[4..4 + ulen]).to_string();
    let mut pos = 4 + ulen;
    if payload.len() < pos + 4 {
        return Err(ResourceError::Transport("short sub".into()));
    }
    let n = u32::from_le_bytes(payload[pos..pos + 4].try_into().unwrap()) as usize;
    pos += 4;
    let mut events = Vec::with_capacity(n);
    for _ in 0..n {
        if payload.len() < pos + 4 {
            return Err(ResourceError::Transport("short sub".into()));
        }
        let elen = u32::from_le_bytes(payload[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if elen > payload.len() - pos {
            return Err(ResourceError::Transport("bad sub event length".into()));
        }
        events.push(String::from_utf8_lossy(&payload[pos..pos + elen]).to_string());
        pos += elen;
    }
    Ok((uri, events))
}
