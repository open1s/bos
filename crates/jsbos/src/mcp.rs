use napi::bindgen_prelude::*;
use napi_derive::napi;

#[napi]
/// A client for a Model Context Protocol server.
pub struct McpClient {
  inner: std::sync::Arc<agent::mcp::McpClient>,
}

#[napi]
impl McpClient {
  #[napi(factory)]
  /// Spawn a stdio MCP server.
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
  /// Create a client for an HTTP MCP endpoint.
  pub fn connect_http(url: String) -> Self {
    let client = agent::mcp::McpClient::connect_http(&url);
    McpClient {
      inner: std::sync::Arc::new(client),
    }
  }

  #[napi]
  /// Initialize the session and return server capabilities.
  pub async fn initialize(&self) -> Result<serde_json::Value> {
    let caps = self
      .inner
      .initialize()
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    serde_json::to_value(caps)
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  /// List the tools the server exposes.
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
  /// Call a tool by name.
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
  /// List the prompts the server exposes.
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
  /// List the resources the server exposes.
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
  /// Read a resource by URI.
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

  #[napi]
  /// Fetch one prompt by name, with optional JSON arguments.
  pub async fn get_prompt(
    &self,
    name: String,
    arguments: Option<String>,
  ) -> Result<serde_json::Value> {
    let arguments = arguments
      .map(|text| {
        serde_json::from_str::<serde_json::Value>(&text)
          .map_err(|e| napi::Error::new(napi::Status::InvalidArg, e.to_string()))
      })
      .transpose()?;
    let result = self
      .inner
      .get_prompt(&name, arguments)
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    serde_json::to_value(&result)
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  /// The capabilities the server reported during initialization.
  pub async fn get_capabilities(&self) -> Result<Option<serde_json::Value>> {
    match self.inner.get_capabilities().await {
      Some(capabilities) => serde_json::to_value(&capabilities)
        .map(Some)
        .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string())),
      None => Ok(None),
    }
  }

  #[napi]
  /// Restart the MCP connection and return the new server capabilities.
  pub async fn restart(&self) -> Result<serde_json::Value> {
    let capabilities = self
      .inner
      .restart()
      .await
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
    serde_json::to_value(&capabilities)
      .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))
  }

  #[napi]
  /// Detailed health status of the MCP connection.
  pub fn health_status(&self) -> serde_json::Value {
    let status = self.inner.health_status();
    let (state, error) = match status.state {
      agent::mcp::client::ConnectionState::Disconnected => ("disconnected", None),
      agent::mcp::client::ConnectionState::Connecting => ("connecting", None),
      agent::mcp::client::ConnectionState::Connected => ("connected", None),
      agent::mcp::client::ConnectionState::Reconnecting => ("reconnecting", None),
      agent::mcp::client::ConnectionState::Failed(ref message) => ("failed", Some(message.clone())),
    };
    serde_json::json!({
        "state": state,
        "error": error,
        "initialized": status.initialized,
        "last_ping": status.last_ping.map(|i| i.elapsed().as_secs_f64()),
        "last_error": status.last_error,
        "restart_count": status.restart_count,
        "idle_duration": status.idle_duration.map(|d| d.as_secs_f64()),
    })
  }
}
