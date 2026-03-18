//! Zenoh queryable wrapper

use std::sync::Arc;
use std::pin::Pin;
use tokio::task::JoinHandle;

use crate::{error::ZenohError, Codec, Session};
use serde::{de::DeserializeOwned, Serialize};
use zenoh::query::Query;

pub struct QueryableWrapper<Q, R>
where
    Q: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
{
    topic: String,
    queryable: Option<Arc<zenoh::query::Queryable<zenoh::handlers::FifoChannelHandler<Query>>>>,
    handler: Option<Handler<Q, R>>,
    codec: Codec,
    _phantom_q: std::marker::PhantomData<Q>,
    _phantom_r: std::marker::PhantomData<R>,
}

type Handler<Q, R> = Box<dyn Fn(Q) -> Pin<Box<dyn std::future::Future<Output = Result<R, ZenohError>> + Send>> + Send + Sync>;

impl<Q, R> QueryableWrapper<Q, R>
where
    Q: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
{
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            queryable: None,
            handler: None,
            codec: Codec::default(),
            _phantom_q: std::marker::PhantomData,
            _phantom_r: std::marker::PhantomData,
        }
    }

    pub fn with_codec(mut self, codec: Codec) -> Self {
        self.codec = codec;
        self
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

    pub async fn run(&self) -> Result<(), ZenohError> {
        let queryable = self.queryable.as_ref().ok_or(ZenohError::NotConnected)?;

        let handler = self
            .handler
            .as_ref()
            .ok_or_else(|| ZenohError::Query("No handler registered".to_string()))?;

        let codec = self.codec;
        let topic = self.topic.clone();

        while let Ok(query) = queryable.recv_async().await {
            Self::handle_query(&query, handler, &codec, &topic).await?;
        }

        Ok(())
    }

    pub fn into_task(mut self) -> Result<JoinHandle<Result<(), ZenohError>>, ZenohError> {
        let queryable = self.queryable.take().ok_or(ZenohError::NotConnected)?;

        let handler = self
            .handler
            .take()
            .ok_or_else(|| ZenohError::Query("No handler registered".to_string()))?;

        let codec = self.codec;
        let topic = self.topic.clone();

        let handle = tokio::spawn(async move {
            while let Ok(query) = queryable.recv_async().await {
                Self::handle_query(&query, &handler, &codec, &topic).await?;
            }
            Ok(())
        });

        Ok(handle)
    }

    async fn handle_query(
        query: &Query,
        handler: &Handler<Q, R>,
        codec: &Codec,
        topic: &str,
    ) -> Result<(), ZenohError> {
        let Some(payload) = query.payload() else {
            return Err(ZenohError::Query("No payload in query".to_string()));
        };

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

    pub fn is_initialized(&self) -> bool {
        self.queryable.is_some()
    }

    pub fn is_running(&self) -> bool {
        self.handler.is_some()
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }
}

impl<Q, R> Clone for QueryableWrapper<Q, R>
where
    Q: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
{
    fn clone(&self) -> Self {
        Self {
            topic: self.topic.clone(),
            queryable: None,
            handler: None,
            codec: self.codec,
            _phantom_q: std::marker::PhantomData,
            _phantom_r: std::marker::PhantomData,
        }
    }
}

impl<Q, R> Drop for QueryableWrapper<Q, R>
where
    Q: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
{
    fn drop(&mut self) {
        self.queryable = None;
        self.handler = None;
    }
}

impl<Q, R> Default for QueryableWrapper<Q, R>
where
    Q: DeserializeOwned + Send + 'static,
    R: Serialize + Send + 'static,
{
    fn default() -> Self {
        Self::new("default/queryable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queryable_wrapper_new() {
        let queryable = QueryableWrapper::<String, String>::new("test/topic");
        assert_eq!(queryable.topic(), "test/topic");
        assert!(!queryable.is_initialized());
        assert!(!queryable.is_running());
    }

    #[test]
    fn test_queryable_wrapper_with_handler() {
        let queryable = QueryableWrapper::<String, String>::new("test/topic")
            .with_handler(|q| async move { Ok(q.to_uppercase()) });

        assert_eq!(queryable.topic(), "test/topic");
        assert!(queryable.is_running());
    }

    #[test]
    fn test_queryable_wrapper_clone() {
        let queryable =
            QueryableWrapper::<i32, i32>::new("test/topic").with_handler(|q| async move { Ok(q * 2) });

        let cloned = queryable.clone();

        assert_eq!(cloned.topic(), "test/topic");
        assert!(!cloned.is_initialized());
        assert!(!cloned.is_running());
    }

    #[test]
    fn test_queryable_wrapper_default() {
        let queryable = QueryableWrapper::<String, String>::default();
        assert_eq!(queryable.topic(), "default/queryable");
    }
}
