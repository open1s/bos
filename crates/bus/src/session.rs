//! Zenoh Bus management

use crate::{Publisher, Session, Subscriber, ZenohError};
use rkyv::api::high::HighDeserializer;
use rkyv::rancor::{Error, Strategy};
use rkyv::ser::allocator::ArenaHandle;
use rkyv::ser::sharing::Share;
use rkyv::ser::Serializer;
use rkyv::util::AlignedVec;
use rkyv::{Archive, Deserialize, Serialize};
use std::ops::Deref;
use std::sync::Arc;
use tokio::task::JoinHandle;
use zenoh::Config;

#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct Bus {
    session: Arc<Session>,
    handles: Arc<tokio::sync::Mutex<Vec<JoinHandle<Result<(), String>>>>>,
}

impl Bus {
    /// Create a Bus from an existing session
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            handles: Arc::new(tokio::sync::Mutex::new(vec![])),
        }
    }

    pub fn session(&self) -> Arc<Session> {
        self.session.clone()
    }

    ///load from config
    pub async fn from(config: BusConfig) -> Self {
        let zenoh_config: Config = config.into();
        let session = zenoh::open(zenoh_config).await.unwrap();
        Self {
            session: Arc::new(session),
            handles: Arc::new(tokio::sync::Mutex::new(vec![])),
        }
    }

    pub async fn subscribe<T, F>(&mut self, topic: &str, mut handler: F)
    where
        F: FnMut(T) + Send + 'static,
        T: Archive + Send + 'static,
        T::Archived: Deserialize<T, HighDeserializer<Error>>,
    {
        let bus = self.session.clone();
        let topic = String::from(topic);

        let handle: tokio::task::JoinHandle<Result<(), String>> = tokio::spawn(async move {
            let mut sub = Subscriber::<T>::new(topic)
                .with_session(bus)
                .await
                .map_err(|e| e.to_string())?;
            while let Some(query) = sub.recv().await {
                handler(query);
            }
            Ok(())
        });
        self.handles.lock().await.push(handle);
    }

    pub async fn publish<T>(&mut self, topic: &str, payload: &T) -> Result<(), ZenohError>
    where
        T: Archive,
        for<'a> T: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,
    {
        let bus = self.session.clone();
        let topic = String::from(topic);

        let publisher = Publisher::new(topic)
            .with_session(bus)
            .map_err(|e| ZenohError::Publisher(e.to_string()))?;
        publisher.publish(payload).await
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        if let Ok(mut handles) = self.handles.try_lock() {
            for handle in handles.drain(..) {
                handle.abort();
            }
        }
    }
}

impl From<Arc<Session>> for Bus {
    fn from(session: Arc<Session>) -> Self {
        Self::new(session)
    }
}

impl From<Bus> for Arc<Session> {
    fn from(val: Bus) -> Self {
        val.session.clone()
    }
}

impl From<Bus> for Session {
    fn from(val: Bus) -> Session {
        val.session.deref().clone()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BusConfig {
    pub mode: String,
    pub connect: Option<Vec<String>>,
    pub listen: Option<Vec<String>>,
    pub peer: Option<Option<String>>,
}

impl BusConfig {
    pub fn new(mode: &str) -> Self {
        Self {
            mode: mode.to_string(),
            connect: None,
            listen: None,
            peer: None,
        }
    }
}

impl From<BusConfig> for zenoh::config::Config {
    fn from(value: BusConfig) -> Config {
        let mut json = if value.mode == "router" {
            r#"{"mode": "router""#.to_string()
        } else {
            r#"{"mode": "peer""#.to_string()
        };

        if let Some(listen) = &value.listen {
            json.push_str(&format!(
                r#", "listen": {{"endpoints": {}}}"#,
                serde_json::to_string(listen).unwrap()
            ));
        }

        if let Some(connect) = &value.connect {
            json.push_str(&format!(
                r#", "connect": {{"endpoints": {}}}"#,
                serde_json::to_string(connect).unwrap()
            ));
        }

        json.push('}');

        zenoh::config::Config::from_json5(&json).unwrap_or_default()
    }
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            mode: "peer".to_string(),
            connect: None,
            listen: None,
            peer: None,
        }
    }
}
