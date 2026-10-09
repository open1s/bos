//! Model-prefix router that dispatches to registered vendor clients.

use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponseResult, ReactContext, ReactSession, TokenStream,
};
use async_trait::async_trait;
use dashmap::DashMap;

/// Routes requests to named vendor clients by model prefix.
///
/// When the primary vendor errors — transport, auth, rate limit, parse —
/// the router walks the registered fallbacks in order and returns the first
/// success, so one dead endpoint never takes the session down. If every
/// provider fails, a single aggregated error names each attempt in order.
pub struct LlmRouter<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> {
    vendors: DashMap<String, Box<dyn LlmClient<S, C>>>,
    fallbacks: Vec<Fallback<S, C>>,
}

/// A fallback endpoint: a pre-built client plus the model id it expects.
struct Fallback<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> {
    /// Bare model id sent on the wire (vendor prefix already stripped).
    send_model: String,
    /// The client to try once the earlier providers failed.
    client: Box<dyn LlmClient<S, C>>,
}

impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmRouter<S, C> {
    /// Create an empty router.
    pub fn new() -> Self {
        Self {
            vendors: DashMap::new(),
            fallbacks: Vec::new(),
        }
    }

    /// Register a vendor under `name`.
    pub fn register_vendor(&mut self, name: String, vendor: Box<dyn LlmClient<S, C>>) {
        self.vendors.insert(name, vendor);
    }

    /// Register a fallback tried whenever the primary vendor errors.
    ///
    /// `model` may carry a `vendor/` prefix; it is stripped with the same
    /// rule as dispatch, so the fallback receives the bare model id its
    /// endpoint expects. Fallbacks are tried in registration order and each
    /// is attempted at most once per request — a broken chain fails fast
    /// instead of looping.
    pub fn register_fallback(&mut self, model: String, client: Box<dyn LlmClient<S, C>>) {
        let send_model = Self::split_model(&model).1.to_string();
        self.fallbacks.push(Fallback { send_model, client });
    }

    fn split_model(model: &str) -> (Option<&str>, &str) {
        if let Some(pos) = model.find('/') {
            let vendor = &model[..pos];
            let model_id = &model[pos + 1..];
            if !vendor.is_empty() && !model_id.is_empty() {
                return (Some(vendor), model_id);
            }
        }
        (None, model)
    }

    /// Aggregate every provider failure into one error naming each attempt.
    fn all_failed(errors: &[LlmError]) -> LlmError {
        LlmError::Other(format!(
            "all {} provider(s) failed: {}",
            errors.len(),
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        ))
    }
}

impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> Default for LlmRouter<S, C> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmClient<S, C>
    for LlmRouter<S, C>
{
    async fn complete(
        &self,
        persona: Option<String>,
        request: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult {
        // Resolve and await the primary inside a block so the DashMap guard
        // drops before any fallback await.
        let primary = {
            let (vendor_id, model_id) = Self::split_model(&request.model);
            let entry = if let Some(vid) = vendor_id {
                self.vendors.get(vid)
            } else {
                let first_key = self.vendors.iter().next().map(|r| r.key().clone());
                first_key.and_then(|k| self.vendors.get(&k))
            };
            if let Some(e) = entry {
                let mut req = request.clone();
                req.model = model_id.to_string();
                e.value()
                    .complete(persona.clone(), req, session, context)
                    .await
            } else {
                Err(LlmError::Other(format!(
                    "Unknown vendor: {}",
                    request.model
                )))
            }
        };
        match primary {
            Ok(response) => Ok(response),
            Err(first) if self.fallbacks.is_empty() => Err(first),
            Err(first) => {
                log::warn!(
                    "llm failover: primary failed ({first}); trying {} fallback(s)",
                    self.fallbacks.len()
                );
                let mut errors = vec![first];
                for fb in &self.fallbacks {
                    let mut req = request.clone();
                    req.model = fb.send_model.clone();
                    match fb
                        .client
                        .complete(persona.clone(), req, session, context)
                        .await
                    {
                        Ok(response) => {
                            log::warn!("llm failover: fallback succeeded ({})", fb.send_model);
                            return Ok(response);
                        }
                        Err(err) => errors.push(err),
                    }
                }
                Err(Self::all_failed(&errors))
            }
        }
    }

    async fn stream_complete(
        &self,
        persona: Option<String>,
        request: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> Result<TokenStream, LlmError> {
        let primary = {
            let (vendor_id, model_id) = Self::split_model(&request.model);
            let entry = if let Some(vid) = vendor_id {
                self.vendors.get(vid)
            } else {
                let first_key = self.vendors.iter().next().map(|r| r.key().clone());
                first_key.and_then(|k| self.vendors.get(&k))
            };
            if let Some(e) = entry {
                let mut req = request.clone();
                req.model = model_id.to_string();
                e.value()
                    .stream_complete(persona.clone(), req, session, context)
                    .await
            } else {
                Ok::<TokenStream, LlmError>(Box::pin(futures::stream::empty()))
            }
        };
        match primary {
            Ok(stream) => Ok(stream),
            Err(first) if self.fallbacks.is_empty() => Err(first),
            Err(first) => {
                log::warn!(
                    "llm failover: primary failed ({first}); trying {} fallback(s)",
                    self.fallbacks.len()
                );
                let mut errors = vec![first];
                for fb in &self.fallbacks {
                    let mut req = request.clone();
                    req.model = fb.send_model.clone();
                    match fb
                        .client
                        .stream_complete(persona.clone(), req, session, context)
                        .await
                    {
                        Ok(stream) => {
                            log::warn!("llm failover: fallback succeeded ({})", fb.send_model);
                            return Ok(stream);
                        }
                        Err(err) => errors.push(err),
                    }
                }
                Err(Self::all_failed(&errors))
            }
        }
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "llm-router"
    }
}
