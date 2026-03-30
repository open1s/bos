//! Zenoh queryable wrapper

use rkyv::{
    api::high::HighDeserializer,
    rancor::{Error, Strategy},
    ser::{allocator::ArenaHandle, sharing::Share, Serializer},
    util::AlignedVec,
    Archive, Deserialize, Serialize,
};

use crate::{error::ZenohError, Codec, Session};
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::task::JoinHandle;
use zenoh::query::Query;

pub struct QueryableWrapper<Q, R>
where
    Q: Archive + 'static,
    R: Archive + Send + 'static,
    Q::Archived: Deserialize<Q, HighDeserializer<Error>>,
    R::Archived: Deserialize<R, HighDeserializer<Error>>,
    for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    for<'a> R: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
{
    topic: String,
    queryable: Option<Arc<zenoh::query::Queryable<zenoh::handlers::FifoChannelHandler<Query>>>>,
    handler: Option<Handler<Q, R>>,
    handle: Option<JoinHandle<Result<(), String>>>,
    started: AtomicBool,
    _phantom_q: std::marker::PhantomData<Q>,
    _phantom_r: std::marker::PhantomData<R>,
}

pub(crate) type Handler<Q, R> = Box<
    dyn Fn(Q) -> Pin<Box<dyn std::future::Future<Output = Result<R, ZenohError>> + Send>>
        + Send
        + Sync,
>;

impl<Q, R> QueryableWrapper<Q, R>
where
    Q: Archive + 'static,
    R: Archive + Send + 'static,
    Q::Archived: Deserialize<Q, HighDeserializer<Error>>,
    R::Archived: Deserialize<R, HighDeserializer<Error>>,
    for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    for<'a> R: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
{
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            queryable: None,
            handler: None,
            started: AtomicBool::new(false),
            handle: None,
            _phantom_q: std::marker::PhantomData,
            _phantom_r: std::marker::PhantomData,
        }
    }

    pub fn with_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(Q) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<R, ZenohError>> + Send + 'static,
    {
        let handler: Handler<Q, R> = Box::new(move |q| Box::pin(handler(q)));
        self.handler = Some(handler);
        self
    }

    pub async fn init(&mut self, session: &Arc<Session>) -> Result<(), ZenohError> {
        let queryable = session
            .declare_queryable(&self.topic)
            .await
            .map_err(|e| ZenohError::Query(e.to_string()))?;

        self.queryable = Some(Arc::new(queryable));
        Ok(())
    }

    pub fn run(&mut self) -> Result<(), ZenohError> {
        let queryable = self.queryable.take().ok_or(ZenohError::NotConnected)?;

        if self
            .started
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            return Err(ZenohError::AlreadyStarted);
        }

        let handler = self.handler.take().ok_or(ZenohError::NotConnected)?;
        let topic = self.topic.clone();

        let handle = tokio::spawn(async move {
            while let Ok(query) = queryable.recv_async().await {
                let _ = Self::handle_query(&query, &handler, &topic).await;
            }
            Ok(())
        });

        self.handle = Some(handle);
        Ok(())
    }

    async fn handle_query(
        query: &Query,
        handler: &Handler<Q, R>,
        topic: &str,
    ) -> Result<(), ZenohError> {
        let Some(payload) = query.payload() else {
            return Err(ZenohError::Query("No payload in query".to_string()));
        };

        let codec = Codec;
        let request: Q = {
            let bytes = payload.to_bytes();
            codec
                .decode(bytes.as_ref())
                .map_err(|e| ZenohError::Serialization(e.to_string()))?
        };

        match handler(request).await {
            Ok(response) => {
                let data = codec
                    .encode(&response)
                    .map_err(|e| ZenohError::Serialization(e.to_string()))?;
                query
                    .reply(topic, data)
                    .await
                    .map_err(|e| ZenohError::Query(e.to_string()))?;
            }
            Err(e) => {
                query
                    .reply_err(format!("Handler error: {}", e))
                    .await
                    .map_err(|e| ZenohError::Query(e.to_string()))?;
            }
        }

        Ok(())
    }

    pub fn topic(&self) -> &str {
        &self.topic
    }
}

impl<Q, R> Drop for QueryableWrapper<Q, R>
where
    Q: Archive + 'static,
    R: Archive + Send + 'static,
    Q::Archived: Deserialize<Q, HighDeserializer<Error>>,
    R::Archived: Deserialize<R, HighDeserializer<Error>>,
    for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    for<'a> R: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
{
    fn drop(&mut self) {
        self.queryable = None;
        self.handler = None;

        if self.started.load(std::sync::atomic::Ordering::Relaxed) {
            self.started
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }

        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

impl<Q, R> Clone for QueryableWrapper<Q, R>
where
    Q: Archive + 'static,
    R: Archive + Send + 'static,
    Q::Archived: Deserialize<Q, HighDeserializer<Error>>,
    R::Archived: Deserialize<R, HighDeserializer<Error>>,
    for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    for<'a> R: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
{
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            queryable: None,
            handler: None,
            started: AtomicBool::new(false),
            handle: None,
            _phantom_q: std::marker::PhantomData,
            _phantom_r: std::marker::PhantomData,
        }
    }
}

impl<Q, R> Default for QueryableWrapper<Q, R>
where
    Q: Archive + 'static,
    R: Archive + Send + 'static,
    Q::Archived: Deserialize<Q, HighDeserializer<Error>>,
    R::Archived: Deserialize<R, HighDeserializer<Error>>,
    for<'a> Q: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    for<'a> R: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
{
    fn default() -> Self {
        Self::new("default/queryable")
    }
}

#[cfg(test)]
mod tests {
    use crate::{Bus, BusConfig, QueryableWrapper, ZenohError};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_queryable_wrapper_with_handler() {
        let config = BusConfig::default();
        let bus = Bus::from(config).await;

        let mut queryable =
            QueryableWrapper::<String, String>::new("test/topic").with_handler(|q| async move {
                println!("IN {:?}", q);
                Ok(q.to_uppercase())
            });

        queryable.init(&bus.clone().into()).await.unwrap();
        let _ = queryable.run();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = crate::query::Query::new("test/topic")
            .with_session(bus.into())
            .await
            .unwrap();

        let results: Result<String, ZenohError> = client.query(&"hello world".to_string()).await;

        assert!(results.is_ok());
        println!("{:?}", results.unwrap());

        let result = client
            .stream_with_handler::<String, String>(&"Hello Zenoh".to_string(), |response| {
                println!("R: {}", response);
                Ok(response)
            })
            .await;

        println!("{:?}", result.unwrap());
    }
}
