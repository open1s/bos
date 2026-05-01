use crate::llm::{
    LlmClient, LlmError, LlmRequest, LlmResponseResult, ReactContext, ReactSession, TokenStream,
};
use async_trait::async_trait;
use dashmap::DashMap;

pub struct LlmRouter<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> {
    vendors: DashMap<String, Box<dyn LlmClient<S, C>>>,
}

impl<S: Send + Sync + ReactSession, C: Send + Sync + ReactContext> LlmRouter<S, C> {
    pub fn new() -> Self {
        Self {
            vendors: DashMap::new(),
        }
    }

    pub fn register_vendor(&mut self, name: String, vendor: Box<dyn LlmClient<S, C>>) {
        self.vendors.insert(name, vendor);
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
        request: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> LlmResponseResult {
        let (vendor_id, model_id) = Self::split_model(&request.model);

        let vendor = if let Some(vid) = vendor_id {
            self.vendors.get(vid)
        } else {
            return Err(LlmError::Other("Vendor not found".to_string()));
        };

        if let Some(v) = vendor {
            let model = model_id.to_string();
            let mut req = request;
            req.model = model;
            v.complete(req, session, context).await
        } else {
            Err(LlmError::Other(format!(
                "Unknown vendor: {}",
                request.model
            )))
        }
    }

    async fn stream_complete(
        &self,
        request: LlmRequest,
        session: &mut S,
        context: &mut C,
    ) -> Result<TokenStream, LlmError> {
        let (vendor_id, model_id) = Self::split_model(&request.model);

        let vendor = if let Some(vid) = vendor_id {
            self.vendors.get(vid)
        } else {
            return Ok(Box::pin(futures::stream::empty()));
        };

        if let Some(v) = vendor {
            let model = model_id.to_string();
            let mut req = request;
            req.model = model;
            v.stream_complete(req, session, context).await
        } else {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    fn supports_tools(&self) -> bool {
        true
    }
    fn provider_name(&self) -> &'static str {
        "llm-router"
    }
}
