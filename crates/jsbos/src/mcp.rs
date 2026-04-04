use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::Arc;

#[napi]
pub struct McpClient {
    inner: std::sync::Arc<agent::mcp::McpClient>,
}

#[napi]
impl McpClient {
    #[napi(factory)]
    pub async fn spawn(command: String, args: Vec<String>) -> Result<Self> {
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let client = agent::mcp::McpClient::spawn(&command, &args_ref)
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(McpClient {
            inner: std::sync::Arc::new(client),
        })
    }

    #[napi(factory)]
    pub fn connect_http(url: String) -> Self {
        let client = agent::mcp::McpClient::connect_http(&url);
        McpClient {
            inner: std::sync::Arc::new(client),
        }
    }

    #[napi]
    pub async fn initialize(&self) -> Result<serde_json::Value> {
        let caps = self
            .inner
            .initialize()
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(serde_json::to_value(caps).unwrap_or(serde_json::Value::Null))
    }

    #[napi]
    pub async fn list_tools(&self) -> Result<Vec<serde_json::Value>> {
        let tools = self
            .inner
            .list_tools()
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        let tools_json: Vec<serde_json::Value> = tools
            .into_iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })
            })
            .collect();
        Ok(tools_json)
    }

    #[napi]
    pub async fn call_tool(&self, name: String, args_json: String) -> Result<serde_json::Value> {
        let args: serde_json::Value = serde_json::from_str(&args_json)
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        let result = self
            .inner
            .call_tool(&name, args)
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(result)
    }

    #[napi]
    pub async fn list_prompts(&self) -> Result<Vec<serde_json::Value>> {
        let prompts = self.inner.list_prompts().await;
        let prompts_json: Vec<serde_json::Value> = prompts
            .into_iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "description": p.description,
                    "arguments": p.arguments,
                })
            })
            .collect();
        Ok(prompts_json)
    }

    #[napi]
    pub async fn list_resources(&self) -> Result<Vec<serde_json::Value>> {
        let resources = self
            .inner
            .list_resources()
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        let resources_json: Vec<serde_json::Value> = resources
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "uri": r.uri,
                    "name": r.name,
                    "description": r.description,
                    "mimeType": r.mime_type,
                })
            })
            .collect();
        Ok(resources_json)
    }

    #[napi]
    pub async fn read_resource(&self, uri: String) -> Result<serde_json::Value> {
        let result = self
            .inner
            .read_resource(&uri)
            .await
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        let contents_json: Vec<serde_json::Value> = result
            .contents
            .into_iter()
            .map(|c| {
                serde_json::json!({
                    "uri": c.uri,
                    "mimeType": c.mime_type,
                    "text": c.text,
                })
            })
            .collect();
        Ok(serde_json::json!({ "contents": contents_json }))
    }
}