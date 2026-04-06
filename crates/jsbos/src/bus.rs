use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::Arc;

pub use bus::Session;

#[napi(object)]
pub struct BusConfig {
    pub mode: String,
    pub connect: Option<Vec<String>>,
    pub listen: Option<Vec<String>>,
    pub peer: Option<String>,
}

impl Default for BusConfig {
    fn default() -> Self {
        let mut loader = config::loader::ConfigLoader::new().discover();
        match loader.load_sync() {
            Ok(config) => {
                let bus_config = config.get("bus");
                Self {
                    mode: bus_config
                        .and_then(|c| c.get("mode"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("peer")
                        .to_string(),
                    connect: bus_config
                        .and_then(|c| c.get("connect"))
                        .and_then(|v| serde_json::from_value(v.clone()).ok()),
                    listen: bus_config
                        .and_then(|c| c.get("listen"))
                        .and_then(|v| serde_json::from_value(v.clone()).ok()),
                    peer: bus_config
                        .and_then(|c| c.get("peer"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                }
            }
            Err(_) => Self {
                mode: "peer".to_string(),
                connect: None,
                listen: None,
                peer: None,
            },
        }
    }
}

impl From<BusConfig> for bus::BusConfig {
    fn from(value: BusConfig) -> Self {
        Self {
            mode: value.mode,
            connect: value.connect,
            listen: value.listen,
            peer: value.peer.map(Some),
        }
    }
}

#[napi]
pub struct Bus {
    inner: Arc<tokio::sync::Mutex<bus::Bus>>,
}

#[napi]
impl Bus {
    #[napi(factory)]
    pub async fn create(config: Option<BusConfig>) -> Result<Bus> {
        let cfg: BusConfig = config.unwrap_or_default();
        let bus = bus::Bus::from(cfg.into()).await;

        Ok(Bus {
            inner: Arc::new(tokio::sync::Mutex::new(bus)),
        })
    }

    #[napi]
    pub async fn session(&self) -> External<Arc<bus::Session>> {
        let guard = self.inner.lock().await;
        External::new(Arc::clone(&guard.session()))
    }

    #[napi]
    pub fn session_id(&self) -> String {
        "session".to_string()
    }

    #[napi]
    pub async fn publish_text(&self, topic: String, payload: String) -> Result<()> {
        let mut guard = self.inner.lock().await;
        guard
            .publish(&topic, &payload)
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(())
    }

    #[napi]
    pub async fn publish_json(&self, topic: String, data: serde_json::Value) -> Result<()> {
        let json_str = data.to_string();
        let mut guard = self.inner.lock().await;
        guard
            .publish(&topic, &json_str)
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(())
    }

    #[napi]
    pub async fn create_publisher(&self, topic: String) -> Result<crate::Publisher> {
        let session = {
            let guard = self.inner.lock().await;
            guard.session().clone()
        };
        let pub_inner = bus::Publisher::new(topic)
            .with_session(session)
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(crate::Publisher { inner: pub_inner })
    }

    #[napi]
    pub async fn create_subscriber(&self, topic: String) -> Result<crate::Subscriber> {
        let session = {
            let guard = self.inner.lock().await;
            guard.session().clone()
        };
        let sub_inner = bus::Subscriber::<String>::new(topic)
            .with_session(session)
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(crate::Subscriber {
            inner: std::sync::Arc::new(tokio::sync::Mutex::new(sub_inner)),
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    #[napi]
    pub async fn create_query(&self, topic: String) -> Result<crate::Query> {
        let session = {
            let guard = self.inner.lock().await;
            guard.session().clone()
        };
        let query_inner = bus::Query::new(topic)
            .with_session(session)
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(crate::Query { inner: query_inner })
    }

    #[napi]
    pub async fn create_queryable(&self, topic: String) -> Result<crate::Queryable> {
        let session = {
            let guard = self.inner.lock().await;
            guard.session().clone()
        };
        let mut wrapper = bus::QueryableWrapper::<String, String>::new(topic);
        wrapper
            .set_handler(|input| async move { Ok(input) })
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        wrapper
            .init(&session)
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(crate::Queryable {
            inner: std::sync::Arc::new(tokio::sync::Mutex::new(wrapper)),
            handler: std::sync::Arc::new(std::sync::Mutex::new(None)),
            stream_handler: std::sync::Arc::new(std::sync::Mutex::new(None)),
        })
    }

    #[napi]
    pub async fn create_caller(&self, name: String) -> Result<crate::Caller> {
        let session = {
            let guard = self.inner.lock().await;
            guard.session().clone()
        };
        Ok(crate::Caller {
            inner: bus::Caller::new(name, Some(session)),
        })
    }

    #[napi]
    pub async fn create_callable(&self, uri: String) -> Result<crate::Callable> {
        let session = {
            let guard = self.inner.lock().await;
            guard.session().clone()
        };
        let callable = bus::Callable::<String, String>::new(&uri, session)
            .with_handler(|input| async move { Ok(input) });
        Ok(crate::Callable::new(callable))
    }
}
