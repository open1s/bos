//! Zenoh Bus management

use std::ops::Deref;
use std::sync::Arc;
use rkyv::api::high::HighDeserializer;
use rkyv::{Archive, Deserialize, Serialize};
use rkyv::rancor::{Error, Strategy};
use rkyv::ser::allocator::ArenaHandle;
use rkyv::ser::Serializer;
use rkyv::ser::sharing::Share;
use rkyv::util::AlignedVec;
use serde_json::from_value;
use tokio::task::JoinHandle;
use zenoh::Config;
use crate::{Publisher, Session, Subscriber, ZenohError};

#[derive(Clone)]
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

    ///load from config
    pub async fn from(config: BusConfig) -> Self{
        let zenoh_config: Config = config.into();
        let session = zenoh::open(zenoh_config).await.unwrap();
        Self{
            session: Arc::new(session),
            handles: Arc::new(tokio::sync::Mutex::new(vec![])),
        }
    }

    pub async fn subscrible<T,F>(&mut self, topic: &str, mut handler: F)
    where
        F: FnMut(T) + Send + 'static,
        T: Archive + Send + 'static,
        T::Archived: Deserialize<T, HighDeserializer<Error>>,{

        let bus = self.session.clone();
        let topic = String::from(topic);

        let handle: tokio::task::JoinHandle<Result<_, String>> = tokio::spawn(async move {
            let mut sub = Subscriber::<T>::new(topic).with_session(bus).await.unwrap();
            while let Some(query) = sub.recv().await {
                handler(query);
            }
            Ok(())
        });
        self.handles.lock().await.push(handle);
    }

    pub async fn publish<T>(&mut self, topic: &str,payload: &T) -> Result<(), ZenohError>
    where
        T: Archive,
        for<'a> T: Serialize<Strategy<Serializer<AlignedVec, ArenaHandle<'a>, Share>, Error>>,{
        let bus = self.session.clone();
        let topic = String::from(topic);

        let publisher = Publisher::new(topic).with_session(bus).unwrap();
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

impl Into<Arc<Session>> for Bus {
    fn into(self) -> Arc<Session> {
        self.session.clone()
    }
}

impl Into<Session>  for Bus {
    fn into(self) -> Session {
        self.session.deref().clone()
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
        Self{
            mode: mode.to_string(),
            connect: None,
            listen: None,
            peer: None,
        }
    }
}

impl Into<zenoh::config::Config> for BusConfig {
    fn into(self) -> Config {
        let zenoh_config_json = serde_json::json!({
            "mode": self.mode,
            "scouting": {
                "multicast": {
                    "enabled": true
                }
            },
            "transport": {
                "unicast": {
                    "accept_timeout": 100
                }
            }
        });

        let config = from_value::<zenoh::config::Config>(zenoh_config_json);
        match config {
            Ok(cfg) => {
                return cfg;
            }
            Err(_) => {
                zenoh::config::Config::default()
            }
        }
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
