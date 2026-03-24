use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use rkyv::api::high::HighDeserializer;
use rkyv::{Archive, Deserialize, Serialize};
use rkyv::rancor::{Error, Strategy};
use rkyv::ser::allocator::ArenaHandle;
use rkyv::ser::Serializer;
use rkyv::ser::sharing::Share;
use rkyv::util::AlignedVec;
use zenoh::Session;
use crate::{QueryableWrapper, ZenohError};

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
    pub fn new(uri: &str,session: Arc<Session>)->Self{
        let inner = QueryableWrapper::<Q,R>::new(
            uri,
        );
        Self{inner:Some(inner), session:session, started: AtomicBool::new(false)}
    }

    pub fn with_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(Q) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<R, ZenohError>> + Send + 'static,{
        self.inner = Some(self.inner.take().unwrap().with_handler(handler));
        self
    }

    pub async fn start(&mut self) -> Result<(), ZenohError> {
        if self.started.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ZenohError::AlreadyStarted);
        }

        let inner = self.inner.take();
        match inner {
            Some(mut inner) => {
                let _ = inner.init(&self.session).await;
                self.started.store(true, std::sync::atomic::Ordering::Relaxed);
                inner.run();
                Ok(())
            }
            None => {
                Err(ZenohError::NotConnected)
            }
        }
    }
}