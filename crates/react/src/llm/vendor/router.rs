use crate::llm::{LlmClient, LlmError, LlmRequest, LlmResponseResult, TokenStream};
use async_trait::async_trait;
use dashmap::DashMap;

pub struct LlmRouter {
    vendors: DashMap<String, Box<dyn LlmClient>>,
}

impl LlmRouter {
    pub fn new() -> Self {
        Self {
            vendors: DashMap::new(),
        }
    }

    pub fn register_vendor(&mut self, name: String, vendor: Box<dyn LlmClient>) {
        self.vendors.insert(name, vendor);
    }

    fn split_model(model: &str) -> (Option<&str>, &str) {
        let parts: Vec<&str> = model.split('/').collect();
        match parts.as_slice() {
            [vendor_id, model_id] if !vendor_id.is_empty() && !model_id.is_empty() => {
                (Some(*vendor_id), *model_id)
            }
            _ => (None, model),
        }
    }
}

impl Default for LlmRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmClient for LlmRouter {
    async fn complete(&self, request: LlmRequest) -> LlmResponseResult {
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
            v.complete(req).await
        } else {
            Err(LlmError::Other(format!(
                "Unknown vendor: {}",
                request.model
            )))
        }
    }

    async fn stream_complete(&self, request: LlmRequest) -> Result<TokenStream, LlmError> {
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
            v.stream_complete(req).await
        } else {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    fn supports_tools(&self) -> bool {
        false
    }
    fn provider_name(&self) -> &'static str {
        "llm-router"
    }
}
