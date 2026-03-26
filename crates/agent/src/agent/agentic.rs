use std::pin::Pin;
use std::sync::Arc;
use futures::{Stream, StreamExt};
use log::info;
use crate::{AgentError, LlmClient, LlmRequest, LlmResponse, OpenAiMessage, StreamToken, Tool, ToolError, ToolRegistry};
use crate::agent::context::MessageContext;
use crate::agent::format_tool_result_content;
use crate::tools::FunctionTool;
use crate::skills::{SkillContent, SkillLoader, SkillInjector, SkillMetadata};
use crate::mcp::{McpClient, McpToolAdapter};

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
    skills: Vec<SkillMetadata>,
    skills_dir: Option<std::path::PathBuf>,
}

impl Agent {
    pub fn new(config: AgentConfig, llm: Arc<dyn LlmClient>) -> Self {
        Self {
            config,
            llm,
            context: MessageContext::new(),
            registry: Some(Arc::new(ToolRegistry::new())),
            skills: Vec::new(),
            skills_dir: None,
        }
    }

    pub fn new_with_registry(config: AgentConfig, llm: Arc<dyn LlmClient>, registry: ToolRegistry) -> Self {
        Self {
            config,
            llm,
            context: MessageContext::new(),
            registry: Some(Arc::new(registry)),
            skills: Vec::new(),
            skills_dir: None,
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

    /// Register a skill with the agent.
    pub fn register_skill(&mut self, skill: SkillMetadata) {
        self.skills.push(skill);
    }

    /// Register skills from a directory using SkillLoader.
    pub fn register_skills_from_dir(&mut self, skills_dir: std::path::PathBuf) -> Result<(), crate::skills::SkillError> {
        let mut loader = SkillLoader::new(skills_dir.clone());
        loader.discover()?;

        for skill_metadata in loader.list() {
            self.register_skill(skill_metadata.clone());
        }

        self.skills_dir = Some(skills_dir);
        Ok(())
    }

    /// Get the skills schemas for sending to LLM.
    pub fn get_skills_schemas(&self) -> Vec<serde_json::Value> {
        self.skills.iter().map(|skill| {
            serde_json::json!({
                "name": skill.name,
                "description": skill.description,
                "category": skill.category.as_str(),
                "tags": skill.tags,
                "requires": skill.requires,
                "provides": skill.provides
            })
        }).collect()
    }

    /// Load full skill content by name.
    pub fn load_skill_content(&self, skill_name: &str) -> Result<SkillContent, crate::skills::SkillError> {
        if let Some(ref skills_dir) = self.skills_dir {
            let mut loader = SkillLoader::new(skills_dir.clone());
            loader.discover()?;
            loader.load(skill_name)
        } else {
            Err(crate::skills::SkillError::NotFound("No skills directory configured".to_string()))
        }
    }

    /// Register MCP tools from an MCP client.
    pub async fn register_mcp_tools(&mut self, client: Arc<McpClient>) -> Result<(), crate::mcp::McpError> {
        let tools = client.list_tools().await?;

        for tool in tools {
            let schema = tool.input_schema.clone();
            let mcp_tool = Arc::new(McpToolAdapter::new(
                client.clone(),
                tool.name.clone(),
                tool.description.clone(),
                schema,
            ));
            self.add_tool(mcp_tool);
        }

        Ok(())
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

        let skills_schemas = self.get_skills_schemas();
        let skills_dir = self.skills_dir.clone();

        tokio::spawn(async move {
            let mut messages = messages;

            loop {
                let mut request_messages = Vec::with_capacity(messages.len() + 1);
                let mut system_prompt = config.system_prompt.clone();

                if !skills_schemas.is_empty() {
                    let skills_json = serde_json::to_string_pretty(&skills_schemas).unwrap();
                    system_prompt = format!("{}\n\nAvailable skills:\n{}\n\nWhen you need to use a skill, respond with a special message in this format: USE_SKILL: skill_name\n\nFor example, if you need to perform calculations, respond with: USE_SKILL: calculator\n\nThe system will then load the skill content and provide it to you.", system_prompt, skills_json);
                }

                request_messages.push(OpenAiMessage::System {
                    content: system_prompt,
                });
                messages.extend_api_format(&mut request_messages);

                let request = LlmRequest {
                    model: config.model.clone(),
                    messages: request_messages,
                    tools: tools_for_request.clone(),
                    temperature: config.temperature,
                    max_tokens: config.max_tokens,
                };

                info!("loop: {:?}", request);
                let stream = llm.stream_complete(request);

                let mut stream = Box::pin(stream);
                let mut tool_call_made = false;
                let mut accumulated_text = String::new();
                let mut skill_requested = false;

                while let Some(token_result) = stream.next().await {
                    match token_result {
                        Ok(token) => {
                            match &token {
                                StreamToken::Text(s) => {
                                    messages.append_assistant_chunk(s);
                                    accumulated_text.push_str(s);

                                    if accumulated_text.contains("USE_SKILL:") {
                                        if let Some(skill_name) = accumulated_text.split("USE_SKILL:").nth(1) {
                                            let skill_name = skill_name.trim().split_whitespace().next().unwrap_or("");
                                            if !skill_name.is_empty() {
                                                skill_requested = true;
                                                if let Some(ref skills_dir) = skills_dir {
                                                    let mut loader = SkillLoader::new(skills_dir.clone());
                                                    if loader.discover().is_ok() {
                                                        if let Ok(skill_content) = loader.load(skill_name) {
                                                            let injector = SkillInjector::new();
                                                            let skill_xml = injector.inject_specific(&[skill_content], &[skill_name]);
                                                            messages.add_assistant(format!("\n\n[SKILL LOADED: {}]\n{}\n", skill_name, skill_xml));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                StreamToken::ToolCall { name, args, id } => {
                                    tool_call_made = true;
                                    let tool_call_id = id.as_ref().map(|s| s.as_str()).unwrap_or_else(|| name.as_str());
                                    messages.add_tool_call(
                                        tool_call_id.to_string(),
                                        name.clone(),
                                        args.clone(),
                                    );
                                    if let Some(ref registry) = tools {
                                        let result = registry.execute(name, args).await;
                                        match result {
                                            Ok(res) => {
                                                messages.add_tool_result(
                                                    tool_call_id.to_string(),
                                                    format_tool_result_content(res),
                                                );
                                            }
                                            Err(e) => {
                                                let _ = tx.send(Err(AgentError::Tool(e))).await;
                                                return;
                                            }
                                        }
                                    }
                                }
                                StreamToken::Done => {
                                    if skill_requested {
                                        skill_requested = false;
                                        continue;
                                    }
                                    if !tool_call_made {
                                        return;
                                    }
                                }
                            }

                            if tx.send(Ok(token)).await.is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(AgentError::Session(e.to_string()))).await;
                            return;
                        }
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
        let skills_schemas = self.get_skills_schemas();
        let skills_dir = self.skills_dir.clone();
        let mut accumulated_text = String::new();

        for _i in 0..MAX_ITERATIONS {
            let mut messages = Vec::with_capacity(self.context.len() + 1);
            let mut system_prompt = self.config.system_prompt.clone();

            if !skills_schemas.is_empty() {
                let skills_json = serde_json::to_string_pretty(&skills_schemas).unwrap();
                system_prompt = format!("{}\n\nAvailable skills:\n{}\n\nWhen you need to use a skill, respond with a special message in this format: USE_SKILL: skill_name\n\nFor example, if you need to perform calculations, respond with: USE_SKILL: calculator\n\nThe system will then load the skill content and provide it to you.", system_prompt, skills_json);
            }

            messages.push(OpenAiMessage::System {
                content: system_prompt,
            });
            self.context.extend_api_format(&mut messages);

            let request = LlmRequest {
                model: self.config.model.clone(),
                messages,
                tools: tools_for_request.clone(),
                temperature: self.config.temperature,
                max_tokens: self.config.max_tokens,
            };
            info!("iteraton:{}, llm request: {:?}",_i, request);
            let response = self.llm.complete(request).await?;
            info!("iteraton:{}, llm response: {:?}",_i, response);

            match response {
                LlmResponse::Text(text) => {
                    accumulated_text.push_str(&text);
                    self.context.add_assistant(text.clone());

                    if text.contains("USE_SKILL:") {
                        if let Some(skill_name) = text.split("USE_SKILL:").nth(1) {
                            let skill_name = skill_name.trim().split_whitespace().next().unwrap_or("");
                            if !skill_name.is_empty() {
                                if let Some(ref skills_dir) = skills_dir {
                                    let mut loader = SkillLoader::new(skills_dir.clone());
                                    if loader.discover().is_ok() {
                                        if let Ok(skill_content) = loader.load(skill_name) {
                                            let injector = SkillInjector::new();
                                            let skill_xml = injector.inject_specific(&[skill_content], &[skill_name]);
                                            self.context.add_assistant(format!("\n\n[SKILL LOADED: {}]\n{}\n", skill_name, skill_xml));
                                            continue;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    break;
                }
                LlmResponse::Patial(part) => {
                    accumulated_text.push_str(&part);
                    self.context.add_assistant(part);
                }
                LlmResponse::ToolCall { name, args, id } => {
                    let tool_call_id = id.unwrap_or_else(|| name.clone());
                    self.context.add_tool_call(
                        tool_call_id.clone(),
                        name.clone(),
                        args.clone(),
                    );
                    if let Some(ref registry) = tools {
                        let result = registry.execute(&name, &args).await?;
                        self.context
                            .add_tool_result(tool_call_id, format_tool_result_content(result));
                    }
                }
                LlmResponse::Done => {
                    break
                },
            }
        }

        Ok(AgentOutput::Text(accumulated_text))
    }
}