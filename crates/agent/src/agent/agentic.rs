use std::pin::Pin;
use std::sync::Arc;
use futures::{Stream, StreamExt};
use crate::{AgentError, LlmClient, LlmRequest, LlmResponse, OpenAiMessage, StreamToken, Tool, ToolError, ToolRegistry};
use crate::agent::context::MessageContext;
use crate::agent::format_tool_result_content;
use crate::tools::FunctionTool;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub system_prompt: String,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub enum AgentOutput {
    Text(String),
    Error(String),
}

pub struct Agent {
    config: AgentConfig,
    llm: Arc<dyn LlmClient>,
    context: MessageContext,
    registry: Option<Arc<ToolRegistry>>,
}

impl Agent {
    pub fn new(config: AgentConfig, llm: Arc<dyn LlmClient>) -> Self {
        Self {
            config,
            llm,
            context: MessageContext::new(),
            registry: Some(Arc::new(ToolRegistry::new())),
        }
    }

    pub fn new_with_registry(config: AgentConfig, llm: Arc<dyn LlmClient>, registry: ToolRegistry) -> Self {
        Self {
            config,
            llm,
            context: MessageContext::new(),
            registry: Some(Arc::new(registry)),
        }
    }

    pub fn context(&self) -> &MessageContext {
        &self.context
    }

    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Register a tool with the internal registry.
    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        if self.registry.is_some() {
            let mut register = self.registry.take().unwrap();
            Arc::make_mut(&mut register).register(tool)?;
            self.registry = Some(register);
        } else {
            let mut registry = ToolRegistry::new();
            registry.register(tool)?;
            self.registry = Some(Arc::new(registry));
        }
        Ok(())
    }

    /// Register a tool, panics on error (convenience method).
    pub fn add_tool(&mut self, tool: Arc<dyn Tool>) {
        self.register_tool(tool).expect("failed to add tool");
    }

    /// Builder-style method to add a tool and return self.
    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.add_tool(tool);
        self
    }

    /// Register a function as a tool using FunctionTool wrapper.
    pub fn register_function<F>(
        &mut self,
        name: &str,
        description: &str,
        schema: serde_json::Value,
        func: F,
    ) -> Result<(), ToolError>
    where
        F: Fn(&serde_json::Value) -> Result<serde_json::Value, ToolError> + Send + Sync + 'static,
    {
        let tool = Arc::new(FunctionTool::new(name, description, schema, func));
        self.register_tool(tool)
    }

    /// Register a numeric function as a tool with auto-generated schema.
    /// The function should accept `&serde_json::Value` and return `Result<serde_json::Value, ToolError>`.
    pub fn register_numeric_function<F>(
        &mut self,
        name: &str,
        description: &str,
        num_params: usize,
        func: F,
    ) -> Result<(), ToolError>
    where
        F: Fn(&serde_json::Value) -> Result<serde_json::Value, ToolError> + Send + Sync + 'static,
    {
        let tool = Arc::new(FunctionTool::numeric(name, description, num_params, func));
        self.register_tool(tool)
    }

    pub async fn run(&mut self, task: &str) -> Result<String, AgentError> {
        self.context.add_user(task.to_string());
        let output = self.run_loop().await?;
        match output {
            AgentOutput::Text(text) => Ok(text),
            AgentOutput::Error(e) => Err(AgentError::Session(e)),
        }
    }

    /// Stream agent execution on a task (no tools).
    /// Returns a stream of tokens as they arrive.
    pub fn stream_run(
        &mut self,
        task: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send>> {
        self.context.add_user(task.to_string());
        self.stream_loop()
    }

    /// Save current agent state to session
    pub async fn save_state(
        &self,
        manager: &crate::session::SessionManager,
    ) -> Result<(), crate::session::SessionError> {
        let metadata =
            crate::session::serializer::SessionSerializer::new_state(self.config.name.clone())
                .metadata;
        let state = crate::session::AgentState {
            agent_id: self.config.name.clone(),
            message_log: self.context.messages.clone(),
            context: serde_json::json!({}),
            metadata,
        };

        manager.update(&self.config.name, state).await
    }

    /// Restore agent state from session
    pub async fn restore_state(
        &mut self,
        manager: &crate::session::SessionManager,
    ) -> Result<(), crate::session::SessionError> {
        let state = manager.get(&self.config.name).await?;
        self.context.messages = state.message_log;
        Ok(())
    }

    /// Auto-save after each turn (optional)
    pub async fn auto_save(&self, manager: &crate::session::SessionManager) {
        if let Err(e) = self.save_state(manager).await {
            eprintln!("Failed to auto-save session: {}", e);
        }
    }

    /// Internal streaming loop - handles token streaming with optional tools.
    fn stream_loop(
        &mut self
    ) -> Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>> + Send>> {
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        let config = self.config.clone();
        let messages = self.context.clone();
        let llm = self.llm.clone();

        let tools = self.registry.clone();
        let tools_for_request = tools.as_ref().map(|t| t.to_openai_format_shared());

        tokio::spawn(async move {
            let mut messages = messages;

            let mut request_messages = Vec::with_capacity(messages.len() + 1);
            request_messages.push(OpenAiMessage::System {
                content: config.system_prompt.clone(),
            });
            messages.extend_api_format(&mut request_messages);

            let request = LlmRequest {
                model: config.model.clone(),
                messages: request_messages,
                tools: tools_for_request.clone(),
                temperature: config.temperature,
                max_tokens: config.max_tokens,
            };

            let stream = llm.stream_complete(request);

            // Pin the stream to use it in a loop
            let mut stream = Box::pin(stream);

            while let Some(token_result) = stream.next().await {
                match token_result {
                    Ok(token) => {
                        // Track message accumulation
                        match &token {
                            StreamToken::Text(s) => {
                                messages.append_assistant_chunk(s);
                            }
                            StreamToken::ToolCall { name, args } => {
                                if let Some(ref registry) = tools {
                                    let result = registry.execute(name, args).await;
                                    match result {
                                        Ok(res) => {
                                            messages.add_tool_result(
                                                name.clone(),
                                                format_tool_result_content(res),
                                            );
                                        }
                                        Err(e) => {
                                            let _ = tx.send(Err(AgentError::Tool(e))).await;
                                        }
                                    }
                                }
                            }
                            StreamToken::Done => {}
                        }

                        if tx.send(Ok(token)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(AgentError::Session(e.to_string()))).await;
                        break;
                    }
                }
            }
        });

        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    async fn run_loop(
        &mut self
    ) -> Result<AgentOutput, AgentError> {
        const MAX_ITERATIONS: usize = 10;
        let tools = self.registry.clone();
        let tools_for_request = tools.as_ref().map(|t| t.to_openai_format_shared());
        let mut accumulated_text = String::new();

        for _ in 0..MAX_ITERATIONS {
            let mut messages = Vec::with_capacity(self.context.len() + 1);
            messages.push(OpenAiMessage::System {
                content: self.config.system_prompt.clone(),
            });
            self.context.extend_api_format(&mut messages);

            let request = LlmRequest {
                model: self.config.model.clone(),
                messages,
                tools: tools_for_request.clone(),
                temperature: self.config.temperature,
                max_tokens: self.config.max_tokens,
            };

            let response = self.llm.complete(request).await?;

            match response {
                LlmResponse::Text(text) => {
                    accumulated_text.push_str(&text);
                    break;
                }
                LlmResponse::Patial(part) => {
                    accumulated_text.push_str(&part);
                    self.context.add_assistant(part);
                }
                LlmResponse::ToolCall { name, args } => {
                    if let Some(ref registry) = tools {
                        let result = registry.execute(&name, &args).await?;
                        self.context
                            .add_tool_result(name, format_tool_result_content(result));
                    }
                }
                LlmResponse::Done => {
                    println!("Llm response: {:?}", accumulated_text);
                    break
                },
            }
        }

        Ok(AgentOutput::Text(accumulated_text))
    }
}