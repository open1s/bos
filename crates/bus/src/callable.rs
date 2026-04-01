use crate::{QueryableWrapper, ZenohError};
use rkyv::api::high::HighDeserializer;
use rkyv::rancor::{Error, Strategy};
use rkyv::ser::allocator::ArenaHandle;
use rkyv::ser::sharing::Share;
use rkyv::ser::Serializer;
use rkyv::util::AlignedVec;
use rkyv::{Archive, Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use zenoh::Session;

pub struct Callable<Q, R>
where
    Q: Archive + 'static,
    R: Archive + Send + 'static,
    Q::Archived: Deserialize<Q, HighDeserializer<Error>>,
    R::Archived: Deserialize<R, HighDeserializer<Error>>,
    for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    for<'a> R: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
{
    inner: Option<QueryableWrapper<Q, R>>,
    session: Arc<Session>,
    started: AtomicBool,
}

impl<Q, R> Callable<Q, R>
where
    Q: Archive + 'static,
    R: Archive + Send + 'static,
    Q::Archived: Deserialize<Q, HighDeserializer<Error>>,
    R::Archived: Deserialize<R, HighDeserializer<Error>>,
    for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    for<'a> R: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
{
    pub fn is_started(&self) -> bool {
        self.started.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl<Q, R> Callable<Q, R>
where
    Q: Archive + 'static,
    R: Archive + Send + 'static,
    Q::Archived: Deserialize<Q, HighDeserializer<Error>>,
    R::Archived: Deserialize<R, HighDeserializer<Error>>,
    for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    for<'a> R: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
{
    pub fn new(uri: &str, session: Arc<Session>) -> Self {
        let inner = QueryableWrapper::<Q, R>::new(uri);
        Self {
            inner: Some(inner),
            session,
            started: AtomicBool::new(false),
        }
    }

    pub fn with_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(Q) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<R, ZenohError>> + Send + 'static,
    {
        self.inner = Some(self.inner.take().unwrap().with_handler(handler));
        self
    }

    pub async fn start(&mut self) -> Result<(), ZenohError> {
        if self.started.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ZenohError::AlreadyStarted);
        }

        let inner = self.inner.as_mut().ok_or(ZenohError::NotConnected)?;
        inner.init(&self.session).await?;
        inner.run()?;
        self.started
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub fn set_handler<F, Fut>(&mut self, handler: F) -> Result<(), ZenohError>
    where
        F: Fn(Q) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<R, ZenohError>> + Send + 'static,
    {
        let inner = self.inner.as_mut().ok_or(ZenohError::NotConnected)?;
        inner.set_handler(handler)
    }

    pub async fn init_and_run(&mut self) -> Result<(), ZenohError> {
        if self.started.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ZenohError::AlreadyStarted);
        }

        let inner = self.inner.as_mut().ok_or(ZenohError::NotConnected)?;
        inner.init(&self.session).await?;
        inner.run()?;
        self.started
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}
