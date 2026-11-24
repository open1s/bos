//! Zenoh subscriber wrapper with simplified API

use rkyv::{Archive, Deserialize, api::high::HighDeserializer, rancor::Error};

use crate::{Codec, Session, error::ZenohError};
use std::sync::Arc;
use tokio::task::JoinHandle;
use zenoh::sample::Sample;

/// Consecutive undecodable payloads one receive loop tolerates before it gives
/// up on the topic.
///
/// A single payload that fails rkyv validation is normal on a shared key-space:
/// another language binding with a different archive layout, a stale schema or
/// any peer can write one. Skipping it keeps the subscription alive, while a
/// topic that never decodes again is reported instead of spinning forever.
pub const MAX_CONSECUTIVE_DECODE_FAILURES: usize = 64;

/// What one receive attempt produced.
///
/// A payload that failed to decode and a closed subscription used to be the
/// same `None`, which made the first malformed sample end every driving loop
/// and silently stop delivery for the life of the process. This keeps the two
/// apart so a caller can skip a bad sample and keep receiving.
pub enum RecvOutcome<T> {
    /// The payload decoded as `T`.
    Payload(T),
    /// The payload did not decode as `T`; the subscription is still live.
    Malformed(String),
    /// The subscription is closed; nothing more will arrive.
    Closed,
}

impl<T> std::fmt::Debug for RecvOutcome<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecvOutcome::Payload(_) => f.write_str("Payload(..)"),
            RecvOutcome::Malformed(error) => f.debug_tuple("Malformed").field(error).finish(),
            RecvOutcome::Closed => f.write_str("Closed"),
        }
    }
}

/// Receives rkyv-encoded messages from a single topic.
pub struct Subscriber<T> {
    topic: String,
    subscriber: Option<zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<Sample>>>,
    started: bool,
    handle: Option<JoinHandle<Result<(), String>>>,
    last_error: Option<String>,
    _phantom: std::marker::PhantomData<T>,
}

impl<T> Subscriber<T>
where
    T: Archive + Send + 'static,
    T::Archived: Deserialize<T, HighDeserializer<Error>>
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, Error>>,
{
    /// Create a new subscriber for the specified topic
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            subscriber: None,
            started: false,
            handle: None,
            last_error: None,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Declare the underlying Zenoh subscription on `session`.
    pub async fn init(&mut self, session: Arc<Session>) -> Result<(), ZenohError> {
        let subscriber = session
            .declare_subscriber(&self.topic)
            .allowed_origin(zenoh::sample::Locality::Any)
            .await
            .map_err(|e| ZenohError::Subscriber(e.to_string()))?;

        self.subscriber = Some(subscriber);
        Ok(())
    }

    /// Associate this subscriber with a session and initialize it
    pub async fn with_session(mut self, session: Arc<Session>) -> Result<Self, ZenohError> {
        self.init(session).await?;
        Ok(self)
    }

    /// Receive the next message, reporting whether its payload decoded.
    ///
    /// Unlike [`recv`](Self::recv), a payload that fails rkyv validation comes
    /// back as [`RecvOutcome::Malformed`] and the subscription stays usable, so
    /// a driving loop can skip the sample instead of ending. Payload integrity
    /// is still the sender's responsibility: `Codec::decode` validates the
    /// archive before rebuilding it.
    pub async fn recv_outcome(&mut self) -> RecvOutcome<T> {
        let Some(subscriber) = self.subscriber.as_mut() else {
            return RecvOutcome::Closed;
        };

        match subscriber.recv_async().await {
            Ok(sample) => match Codec.decode::<T>(sample.payload().to_bytes().as_ref()) {
                Ok(value) => RecvOutcome::Payload(value),
                Err(error) => RecvOutcome::Malformed(error.to_string()),
            },
            Err(error) => {
                self.last_error = Some(error.to_string());
                RecvOutcome::Closed
            }
        }
    }

    /// Receive the next message that decodes as `T`.
    ///
    /// A payload that does not decode is skipped - up to
    /// [`MAX_CONSECUTIVE_DECODE_FAILURES`] in a row - so one malformed sample
    /// no longer end the loop that drives this subscriber. `None` means the
    /// subscriber is not initialized, the subscription closed, or the topic
    /// produced nothing decodable that many times in a row.
    pub async fn recv(&mut self) -> Option<T> {
        let mut consecutive_failures = 0usize;

        loop {
            match self.recv_outcome().await {
                RecvOutcome::Payload(value) => return Some(value),
                RecvOutcome::Malformed(error) => {
                    consecutive_failures += 1;
                    self.last_error = Some(error);
                    if consecutive_failures >= MAX_CONSECUTIVE_DECODE_FAILURES {
                        return None;
                    }
                }
                RecvOutcome::Closed => return None,
            }
        }
    }

    /// Why the last receive attempt ended, when Zenoh reported a cause.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Spawn a task that invokes `handler` for every decoded message.
    ///
    /// Undecodable payloads are skipped, up to
    /// [`MAX_CONSECUTIVE_DECODE_FAILURES`] in a row. When the subscription
    /// ends, either because Zenoh closed the channel or because the topic
    /// stopped decoding, the task resolves to `Err` naming the topic and the
    /// cause; the handle used to resolve to `Ok(())`, so a caller checking
    /// only the handle saw success. Await it with [`join`](Self::join).
    pub async fn run<F>(&mut self, handler: F) -> Result<(), ZenohError>
    where
        F: Fn(T) + std::marker::Send + 'static,
    {
        let subscriber = self.subscriber.take().ok_or(ZenohError::NotConnected)?;
        let topic = self.topic.clone();

        self.started = true;
        let handle: tokio::task::JoinHandle<Result<(), String>> = tokio::spawn(async move {
            let mut consecutive_failures = 0usize;

            loop {
                match subscriber.recv_async().await {
                    Ok(sample) => match Codec.decode::<T>(sample.payload().to_bytes().as_ref()) {
                        Ok(value) => {
                            consecutive_failures = 0;
                            handler(value);
                        }
                        Err(error) => {
                            consecutive_failures += 1;
                            if consecutive_failures >= MAX_CONSECUTIVE_DECODE_FAILURES {
                                return Err(format!(
                                    "{topic} delivered {consecutive_failures} undecodable payloads in a row: {error}"
                                ));
                            }
                        }
                    },
                    Err(error) => {
                        return Err(format!("{topic} subscription ended: {error}"));
                    }
                }
            }
        });

        self.handle = Some(handle);
        Ok(())
    }

    /// Await the delivery task started by [`run`](Self::run).
    ///
    /// Returns `None` when no task was started. A task that stopped because the
    /// subscription ended, or because the topic never decoded again, reports
    /// `Err` with the reason.
    pub async fn join(&mut self) -> Option<Result<(), String>> {
        let topic = self.topic.clone();
        let handle = self.handle.take()?;
        self.started = false;

        Some(match handle.await {
            Ok(result) => result,
            Err(error) => Err(format!("delivery task for {topic} ended: {error}")),
        })
    }

    /// Receive with a timeout
    pub async fn recv_with_timeout(&mut self, timeout: std::time::Duration) -> Option<T> {
        tokio::time::timeout(timeout, self.recv())
            .await
            .unwrap_or_default()
    }

    /// Stop the subscriber by dropping the underlying Zenoh subscription.
    /// Any pending `recv` will return `None`.
    pub fn stop(&mut self) {
        self.subscriber = None;
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }

    /// Get the topic
    pub fn topic(&self) -> &str {
        &self.topic
    }
}

impl<T> Drop for Subscriber<T> {
    fn drop(&mut self) {
        if self.started {
            self.started = false;
        }

        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

impl<T> Clone for Subscriber<T> {
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            subscriber: None,
            started: false,
            handle: None,
            last_error: None,
            _phantom: self._phantom,
        }
    }
}

impl<T> Default for Subscriber<T>
where
    T: Archive + Send + 'static,
    T::Archived: Deserialize<T, HighDeserializer<Error>>
        + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, Error>>,
{
    fn default() -> Self {
        Self::new("default/subscriber")
    }
}
