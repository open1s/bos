//! In-process transport: dispatches directly to a [`Dispatcher`]. Useful for
//! local-first delivery and for tests (no network).

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;

use super::Dispatcher;
use super::Transport;
use super::{ChunkStream, ChunkWriter};
use crate::action::{ResourceAction, ResourceEvent, ResourceOutput};
use crate::error::Result;
use crate::meta::{ResourceInfo, ResourceType};

/// A transport that calls a local dispatcher. The `agent` identity is fixed at
/// construction (typically the local owner handle id).
pub struct InProcessTransport {
    dispatcher: Arc<dyn Dispatcher>,
    agent: String,
}

impl InProcessTransport {
    pub fn new(dispatcher: Arc<dyn Dispatcher>, agent: impl Into<String>) -> Self {
        Self {
            dispatcher,
            agent: agent.into(),
        }
    }
}

#[async_trait]
impl Transport for InProcessTransport {
    async fn invoke(&self, uri: &str, action: ResourceAction) -> Result<ResourceOutput> {
        self.dispatcher.dispatch(&self.agent, uri, action).await
    }

    async fn subscribe(
        &self,
        uri: &str,
        events: Vec<String>,
    ) -> Result<BoxStream<'static, ResourceEvent>> {
        self.dispatcher.subscribe(&self.agent, uri, events).await
    }

    async fn list(&self, agent: &str, kind: Option<ResourceType>) -> Result<Vec<ResourceInfo>> {
        self.dispatcher.list(agent, kind).await
    }

    async fn resolve(&self, agent: &str, uri: &str) -> Result<Option<ResourceInfo>> {
        self.dispatcher.resolve(agent, uri).await
    }

    async fn read_stream(
        &self,
        uri: &str,
        offset: u64,
        len: Option<u64>,
    ) -> Result<ChunkStream> {
        self.dispatcher
            .read_stream(&self.agent, uri, offset, len)
            .await
    }

    async fn write_stream(&self, uri: &str, offset: u64) -> Result<Box<dyn ChunkWriter>> {
        self.dispatcher.write_stream(&self.agent, uri, offset).await
    }
}