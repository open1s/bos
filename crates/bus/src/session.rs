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
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use zenoh::Config;

#[derive(Clone)]
#[allow(clippy::type_complexity)]
/// An open Zenoh session plus the background tasks the bus spawned.
pub struct Bus {
    session: Arc<Session>,
    handles: Arc<Mutex<Vec<JoinHandle<Result<(), String>>>>>,
}

impl Bus {
    /// Create a Bus from an existing session
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            handles: Arc::new(Mutex::new(vec![])),
        }
    }

    /// The underlying Zenoh session.
    pub fn session(&self) -> Arc<Session> {
        self.session.clone()
    }

    fn abort_handles(&self) {
        // The critical section only drains a Vec; no I/O happens while the
        // lock is held, so a blocking std mutex is appropriate here and lets
        // Drop abort deterministically.
        let mut handles = match self.handles.lock() {
            Ok(handles) => handles,
            Err(poisoned) => poisoned.into_inner(),
        };
        for handle in handles.drain(..) {
            handle.abort();
        }
    }

    fn push_handle(&self, handle: JoinHandle<Result<(), String>>) {
        let mut handles = match self.handles.lock() {
            Ok(handles) => handles,
            Err(poisoned) => poisoned.into_inner(),
        };
        handles.push(handle);
    }

    /// Close the bus, aborting all running tasks and releasing resources.
    /// After close(), the Bus is still usable but may require re-init.
    pub async fn close(&mut self) {
        self.abort_handles();
        let _ = self.session.close().await;
    }

    /// Open a bus from config, reporting why Zenoh refused it.
    ///
    /// A configuration Zenoh rejects - a bad TLS path, a listen endpoint that
    /// is already bound - is an error here rather than a silent fallback to
    /// the defaults, because a node that asked for TLS must not quietly run
    /// without it.
    pub async fn try_from(config: BusConfig) -> Result<Self, String> {
        let zenoh_config = zenoh::config::Config::from_json5(&config.to_zenoh_json())
            .map_err(|error| format!("zenoh rejected the generated config: {error}"))?;
        let session = zenoh::open(zenoh_config)
            .await
            .map_err(|error| format!("zenoh refused to open a session: {error}"))?;
        Ok(Self {
            session: Arc::new(session),
            handles: Arc::new(Mutex::new(vec![])),
        })
    }

    /// Open a bus from config.
    ///
    /// # Panics
    ///
    /// Panics if Zenoh cannot open a session with the resulting configuration
    /// (for example when a listen endpoint is already bound). Prefer
    /// [`Bus::try_from`] where the failure must be reported.
    pub async fn from(config: BusConfig) -> Self {
        Self::try_from(config)
            .await
            .expect("a zenoh session opens with the generated configuration")
    }

    /// Subscribe to `topic`, calling `handler` for every decoded message.
    pub async fn subscribe<T, F>(&self, topic: &str, mut handler: F)
    where
        F: FnMut(T) + Send + 'static,
        T: Archive + Send + 'static,
        T::Archived: Deserialize<T, HighDeserializer<Error>>
            + for<'a> rkyv::bytecheck::CheckBytes<rkyv::api::high::HighValidator<'a, Error>>,
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
        self.push_handle(handle);
    }

    /// Publish one rkyv-encoded value to `topic`.
    pub async fn publish<T>(&self, topic: &str, payload: &T) -> Result<(), ZenohError>
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
        // The handles are shared by every clone of this bus. Only the last
        // owner may abort them: dropping a temporary clone (such as the
        // bus.clone().into() pattern used to obtain a session) must not
        // cancel subscriptions another owner still relies on.
        if Arc::strong_count(&self.handles) == 1 {
            self.abort_handles();
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

/// Transport TLS settings, rendered into Zenoh's `transport.link.tls`.
///
/// The same certificate and key are used for listening and connecting: a
/// federation node usually presents one identity to its peers. Paths are
/// passed through as written; Zenoh reads the files when the session opens.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TlsConfig {
    /// CA bundle that verifies the peer.
    pub root_ca_certificate: Option<String>,
    /// Certificate this node presents.
    pub certificate: Option<String>,
    /// Private key for [`TlsConfig::certificate`].
    pub private_key: Option<String>,
    /// Require a client certificate from the peer.
    pub enable_mtls: Option<bool>,
    /// Check the peer certificate's name against the endpoint.
    pub verify_name_on_connect: Option<bool>,
}
/// Connection settings used to open a Zenoh session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BusConfig {
    /// Zenoh mode, usually `peer` or `router`.
    pub mode: String,
    /// Endpoints to connect to.
    pub connect: Option<Vec<String>>,
    /// Endpoints to listen on.
    pub listen: Option<Vec<String>>,
    /// Zenoh id override, emitted as id in the generated configuration.
    ///
    /// When set, the value must be a valid Zenoh id (at most 16 bytes).
    pub peer: Option<Option<String>>,
    /// Transport TLS settings, when the session must be encrypted.
    pub tls: Option<TlsConfig>,
}

impl BusConfig {
    /// Create a config with just `mode` set.
    pub fn new(mode: &str) -> Self {
        Self {
            mode: mode.to_string(),
            connect: None,
            listen: None,
            peer: None,
            tls: None,
        }
    }
}

impl BusConfig {
    /// Render this configuration as the JSON5 document handed to Zenoh.
    fn to_zenoh_json(&self) -> String {
        // Pass the mode through so valid Zenoh modes other than peer/router
        // (for example client) are not silently rewritten.
        let mode = if self.mode.is_empty() {
            "peer"
        } else {
            self.mode.as_str()
        };
        let mut json = format!(
            r#"{{"mode": {}"#,
            serde_json::to_string(mode).expect("a string always serializes")
        );

        if let Some(listen) = &self.listen {
            json.push_str(&format!(
                r#", "listen": {{"endpoints": {}}}"#,
                serde_json::to_string(listen).expect("a list of strings always serializes")
            ));
        }

        if let Some(connect) = &self.connect {
            json.push_str(&format!(
                r#", "connect": {{"endpoints": {}}}"#,
                serde_json::to_string(connect).expect("a list of strings always serializes")
            ));
        }

        if let Some(tls) = &self.tls {
            let mut block = String::from(r#""transport": {"link": {"tls": {"#);
            let mut fields: Vec<String> = Vec::new();
            if let Some(value) = &tls.root_ca_certificate {
                fields.push(format!(
                    r#""root_ca_certificate": {}"#,
                    serde_json::to_string(value).expect("a string always serializes")
                ));
            }
            if let Some(value) = &tls.certificate {
                for key in ["listen_certificate", "connect_certificate"] {
                    fields.push(format!(
                        r#""{key}": {}"#,
                        serde_json::to_string(value).expect("a string always serializes")
                    ));
                }
            }
            if let Some(value) = &tls.private_key {
                for key in ["listen_private_key", "connect_private_key"] {
                    fields.push(format!(
                        r#""{key}": {}"#,
                        serde_json::to_string(value).expect("a string always serializes")
                    ));
                }
            }
            if let Some(value) = tls.enable_mtls {
                fields.push(format!("\"enable_mtls\": {value}"));
            }
            if let Some(value) = tls.verify_name_on_connect {
                fields.push(format!("\"verify_name_on_connect\": {value}"));
            }
            block.push_str(&fields.join(", "));
            block.push_str("}}}");
            json.push_str(", ");
            json.push_str(&block);
        }

        if let Some(Some(peer)) = &self.peer {
            // Only emit a syntactically valid Zenoh id: an invalid value would
            // make Zenoh reject the whole document and silently discard the
            // listen/connect endpoints as well.
            if zenoh::config::ZenohId::from_str(peer).is_ok() {
                json.push_str(&format!(
                    r#", "id": {}"#,
                    serde_json::to_string(peer).expect("a string always serializes")
                ));
            }
        }

        json.push('}');
        json
    }
}

impl From<BusConfig> for zenoh::config::Config {
    fn from(value: BusConfig) -> Config {
        // A configuration Zenoh rejects falls back to the Zenoh defaults; the
        // generated document is exercised by the unit tests below.
        zenoh::config::Config::from_json5(&value.to_zenoh_json()).unwrap_or_default()
    }
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            mode: "peer".to_string(),
            connect: None,
            listen: None,
            peer: None,
            tls: None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn config_json(value: BusConfig) -> serde_json::Value {
        let rendered = value.to_zenoh_json();
        let config = zenoh::config::Config::from_json5(&rendered)
            .unwrap_or_else(|e| panic!("generated config rejected by zenoh: {e}\n{rendered}"));
        serde_json::to_value(&config).expect("zenoh config serializes")
    }

    #[test]
    fn test_mode_is_passed_through() {
        let json = config_json(BusConfig::new("client"));
        assert_eq!(json.get("mode").and_then(|m| m.as_str()), Some("client"));
    }

    #[test]
    fn test_empty_mode_falls_back_to_peer() {
        let json = config_json(BusConfig::new(""));
        assert_eq!(json.get("mode").and_then(|m| m.as_str()), Some("peer"));
    }

    #[test]
    fn test_tls_settings_are_rendered_and_accepted() {
        let config = BusConfig {
            tls: Some(TlsConfig {
                root_ca_certificate: Some("/ca.pem".to_string()),
                certificate: Some("/cert.pem".to_string()),
                private_key: Some("/key.pem".to_string()),
                enable_mtls: Some(true),
                verify_name_on_connect: Some(true),
            }),
            ..BusConfig::default()
        };

        let json = config_json(config);
        let tls = &json["transport"]["link"]["tls"];

        assert_eq!(tls["root_ca_certificate"], "/ca.pem");
        assert_eq!(tls["listen_certificate"], "/cert.pem");
        assert_eq!(tls["connect_certificate"], "/cert.pem");
        assert_eq!(tls["listen_private_key"], "/key.pem");
        assert_eq!(tls["connect_private_key"], "/key.pem");
        assert_eq!(tls["enable_mtls"], true);
        assert_eq!(tls["verify_name_on_connect"], true);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn test_a_tls_failure_is_reported_not_swallowed() {
        // Zenoh loads TLS material lazily, so a missing file alone is not an
        // error; a TLS listener that actually needs the material is.
        let tls_listener = BusConfig {
            listen: Some(vec!["tls/127.0.0.1:0".to_string()]),
            tls: Some(TlsConfig {
                root_ca_certificate: Some("/nonexistent/ca.pem".to_string()),
                certificate: Some("/nonexistent/cert.pem".to_string()),
                private_key: Some("/nonexistent/key.pem".to_string()),
                ..TlsConfig::default()
            }),
            ..BusConfig::default()
        };
        assert!(
            Bus::try_from(tls_listener).await.is_err(),
            "a TLS listener without usable material must not open"
        );

        // A malformed document is an error, never a silent fallback to the
        // Zenoh defaults.
        let bad_endpoint = BusConfig {
            listen: Some(vec!["not-an-endpoint".to_string()]),
            ..BusConfig::default()
        };
        assert!(
            Bus::try_from(bad_endpoint).await.is_err(),
            "zenoh's refusal is reported, not swallowed"
        );
    }
    #[test]
    fn test_peer_id_becomes_zenoh_id() {
        let config = BusConfig {
            peer: Some(Some("abc".to_string())),
            ..BusConfig::default()
        };
        let json = config_json(config);
        let expected =
            serde_json::to_value(zenoh::config::ZenohId::from_str("abc").expect("valid zenoh id"))
                .expect("id serializes");
        assert_eq!(json.get("id"), Some(&expected));
    }

    #[test]
    fn test_unset_peer_leaves_id_unset() {
        let json = config_json(BusConfig::default());
        assert!(json.get("id").is_some_and(|id| id.is_null()));
    }

    #[test]
    fn test_invalid_peer_id_is_ignored_without_dropping_endpoints() {
        let config = BusConfig {
            peer: Some(Some("not a zenoh id".to_string())),
            listen: Some(vec!["tcp/127.0.0.1:0".to_string()]),
            ..BusConfig::default()
        };
        let json = config_json(config);
        assert!(
            json.get("id").is_some_and(|id| id.is_null()),
            "invalid ids are not emitted: {json}"
        );
        assert!(
            json.to_string().contains("tcp/127.0.0.1:0"),
            "valid endpoints survive an invalid peer id: {json}"
        );
    }
}
