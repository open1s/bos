//! Skills & MCP Demo - Phase 4 Plan 03
//!
//! Demonstrates skill loading, composition, injection, and MCP tool integration.

use agent::skills::{SkillLoader, SkillInjector, SkillContent};
use agent::mcp::{McpClient, McpToolAdapter};
use agent::tools::{ToolRegistry, Tool};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "demo-skills-mcp")]
#[command(about = "Skills & MCP Demo - BrainOS Agent Framework")]
struct Args {
    #[command(subcommand)]
    command: Command,

    /// Directory containing skill definitions
    #[arg(long, default_value = "./skills")]
    skills_dir: PathBuf,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List all discovered skills with metadata
    ListSkills,
    /// Compose multiple skills and check for conflicts
    ComposeSkills,
    /// Inject skills into agent system prompt (3 formats)
    InjectSkills,
    /// Connect to an MCP server and list tools
    McpConnect {
        /// MCP server name (e.g., 'everything', 'tree-sitter')
        server: String,
    },
    /// Create MCP tool adapters and register in ToolRegistry
    McpAdapter {
        /// MCP server name (e.g., 'everything', 'tree-sitter')
        server: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    println!("╔══════════════════════════════════════╗");
    println!("║   Skills & MCP Demo - Phase 4.03    ║");
    println!("╚══════════════════════════════════════╝\n");

    match args.command {
        Command::ListSkills => list_skills(args.skills_dir).await,
        Command::ComposeSkills => compose_skills(args.skills_dir).await,
        Command::InjectSkills => inject_skills(args.skills_dir).await,
        Command::McpConnect { server } => mcp_connect(server).await,
        Command::McpAdapter { server } => mcp_adapter(server).await,
    }
}

/// List all discovered skills with their metadata
async fn list_skills(dir: PathBuf) -> Result<()> {
    println!("📁 Discovering skills in: {:?}\n", dir);

    let mut loader = SkillLoader::new(dir.clone());
    let skills = loader.discover()?;

    if skills.is_empty() {
        println!("⚠️  No skills found. Create skills/<name>/SKILL.md files.");
        return Ok(());
    }

    println!("✓ Found {} skill(s):\n", skills.len());

    for skill in &skills {
        println!("┌─────────────────────────────────────");
        println!("│ 📦 {} ({})", skill.name, skill.category.display_name());
        println!("├─────────────────────────────────────");
        println!("│ Description: {}", skill.description);
        println!("│ Version:     {}", skill.version.display());
        println!("│ Path:        {:?}", skill.path);
        
        if !skill.tags.is_empty() {
            println!("│ Tags:        {}", skill.tags.join(", "));
        }
        
        if !skill.requires.is_empty() {
            println!("│ Requires:    {}", skill.requires.join(", "));
        }
        
        if !skill.provides.is_empty() {
            println!("│ Provides:    {}", skill.provides.join(", "));
        }
        
        if let Some(ref author) = skill.author {
            println!("│ Author:      {}", author);
        }
        
        println!("└─────────────────────────────────────\n");
    }

    // Show statistics
    let stats = loader.stats();
    println!("📊 Statistics:");
    println!("{}", stats);

    // Validate dependencies
    println!("\n🔍 Validating dependencies...");
    match loader.validate_all() {
        Ok(()) => println!("✓ All skill dependencies satisfied"),
        Err(e) => println!("✗ Validation error: {}", e),
    }

    Ok(())
}

/// Compose multiple skills and check for tool conflicts
async fn compose_skills(dir: PathBuf) -> Result<()> {
    println!("🔧 Composing multiple skills...\n");

    let mut loader = SkillLoader::new(dir);
    let skills = loader.discover()?;

    if skills.is_empty() {
        println!("⚠️  No skills to compose");
        return Ok(());
    }

    // Load all skill contents
    let mut skill_contents: Vec<SkillContent> = Vec::new();
    for metadata in &skills {
        match loader.load(&metadata.name) {
            Ok(content) => {
                println!("✓ Loaded: {} ({} bytes)", metadata.name, content.instructions.len());
                skill_contents.push(content);
            }
            Err(e) => {
                println!("✗ Failed to load {}: {}", metadata.name, e);
            }
        }
    }

    println!("\n📋 Loaded {} skill(s)\n", skill_contents.len());

    // Check for tool conflicts by examining provides fields
    let mut provided_capabilities: std::collections::HashMap<String, Vec<String>> = 
        std::collections::HashMap::new();
    
    for skill in &skills {
        for cap in &skill.provides {
            provided_capabilities
                .entry(cap.clone())
                .or_default()
                .push(skill.name.clone());
        }
    }

    // Report conflicts
    let mut has_conflicts = false;
    for (cap, providers) in &provided_capabilities {
        if providers.len() > 1 {
            has_conflicts = true;
            println!("⚠️  Conflict: '{}' provided by: {}", cap, providers.join(", "));
        }
    }

    if !has_conflicts {
        println!("✓ No capability conflicts detected");
    }

    // Show dependency graph
    println!("\n📊 Dependency Graph:");
    let graph = loader.dependency_graph();
    for (name, deps) in &graph {
        if deps.is_empty() {
            println!("  {} → (no dependencies)", name);
        } else {
            println!("  {} → [{}]", name, deps.join(", "));
        }
    }

    // Validate all dependencies
    println!("\n🔍 Validating skill composition...");
    match loader.validate_all() {
        Ok(()) => println!("✓ All dependencies satisfied"),
        Err(e) => println!("✗ Validation error: {}", e),
    }

    Ok(())
}

/// Inject skills into agent system prompt using 3 formats
async fn inject_skills(dir: PathBuf) -> Result<()> {
    println!("💉 Injecting skills into agent prompt...\n");

    let mut loader = SkillLoader::new(dir);
    let skills = loader.discover()?;

    if skills.is_empty() {
        println!("⚠️  No skills to inject");
        return Ok(());
    }

    // Load all skill contents
    let skill_contents: Vec<SkillContent> = skills
        .iter()
        .filter_map(|m| loader.load(&m.name).ok())
        .collect();

    println!("✓ Loaded {} skill(s) for injection\n", skill_contents.len());

    let injector = SkillInjector::new();

    // Test Compact Format
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📝 COMPACT FORMAT (minimal tokens)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    
    let compact = injector.inject_available(&skills);
    println!("{}", compact);
    let compact_stats = injector.injected_stats(&compact);
    println!("\n📊 Stats: {} skills, {} chars, {} lines\n", 
        compact_stats.skill_count, compact_stats.char_count, compact_stats.line_count);

    // Test Standard Format
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📝 STANDARD FORMAT (balanced detail)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    
    let standard = injector.inject_specific(&skill_contents, 
        &skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>());
    println!("{}", standard);
    let standard_stats = injector.injected_stats(&standard);
    println!("\n📊 Stats: {} skills, {} chars, {} lines\n", 
        standard_stats.skill_count, standard_stats.char_count, standard_stats.line_count);

    // Test Verbose Format (with full instructions)
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📝 VERBOSE FORMAT (full instructions)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    
    let verbose = injector.inject_by_category(&skill_contents, 
        agent::skills::SkillCategory::Communication);
    println!("{}", verbose);
    let verbose_stats = injector.injected_stats(&verbose);
    println!("\n📊 Stats: {} skills, {} chars, {} lines\n", 
        verbose_stats.skill_count, verbose_stats.char_count, verbose_stats.line_count);

    // Simulate injection into agent system prompt
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("🤖 SIMULATED AGENT SYSTEM PROMPT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let mut system_prompt = String::from("You are a helpful AI assistant.\n\n");
    system_prompt.push_str("# Available Skills\n\n");
    system_prompt.push_str(&standard);
    system_prompt.push_str("\n\n# Instructions\n\n");
    system_prompt.push_str("Use the skills above to guide your responses. ");
    system_prompt.push_str("Apply relevant skills based on the user's request.\n");

    println!("{}", system_prompt);
    println!("\n📊 Total system prompt size: {} bytes", system_prompt.len());

    Ok(())
}

/// Connect to an MCP server and list available tools
async fn mcp_connect(server: String) -> Result<()> {
    println!("🔌 Connecting to MCP server: {}\n", server);

    // Determine command and args based on server name
    let (command, args): (&str, Vec<&str>) = if server == "everything" {
        ("mcp-everything", vec![])
    } else if server == "tree-sitter" {
        ("tree-sitter", vec!["--stdio"])
    } else {
        // Use the server name directly
        (server.as_str(), vec![])
    };

    // Check if server is available in PATH
    println!("🔍 Checking if '{}' is in PATH...", command);
    match tokio::process::Command::new(command)
        .arg("--version")
        .output()
        .await
    {
        Ok(output) => {
            if output.status.success() {
                println!("✓ Server found in PATH");
            } else {
                println!("⚠️  Server found but --version failed");
            }
        }
        Err(_) => {
            println!("⚠️  Server '{}' not found in PATH", command);
            println!("   Install it or ensure it's in your PATH");
            println!("   Skipping MCP connection demo");
            return Ok(());
        }
    }

    println!("\n🚀 Spawning MCP server...");
    let client = match McpClient::spawn(command, &args).await {
        Ok(c) => {
            println!("✓ Server spawned successfully");
            c
        }
        Err(e) => {
            println!("✗ Failed to spawn server: {}", e);
            return Ok(());
        }
    };

    println!("\n📡 Initializing protocol...");
    let capabilities = match client.initialize().await {
        Ok(caps) => {
            println!("✓ Protocol initialized");
            caps
        }
        Err(e) => {
            println!("✗ Failed to initialize: {}", e);
            return Ok(());
        }
    };

    println!("\n📋 Server Capabilities:");
    println!("   Tools:     {}", if capabilities.tools { "✓" } else { "✗" });
    println!("   Resources: {}", if capabilities.resources { "✓" } else { "✗" });
    println!("   Prompts:   {}", if capabilities.prompts { "✓" } else { "✗" });

    if capabilities.tools {
        println!("\n🔧 Listing tools...");
        let tools = match client.list_tools().await {
            Ok(t) => t,
            Err(e) => {
                println!("✗ Failed to list tools: {}", e);
                return Ok(());
            }
        };

        println!("✓ Found {} tool(s)\n", tools.len());

        for tool in &tools {
            println!("┌─────────────────────────────────────");
            println!("│ 🔧 {}", tool.name);
            println!("├─────────────────────────────────────");
            println!("│ {}", tool.description);
            println!("│");
            println!("│ Input Schema:");
            println!("│ {}", serde_json::to_string_pretty(&tool.input_schema)?);
            println!("└─────────────────────────────────────\n");
        }
    } else {
        println!("\n⚠️  Server does not support tools");
    }

    if capabilities.resources {
        println!("\n📚 Listing resources...");
        let resources = client.list_resources().await;

        match resources {
            Ok(resource_list) => {
                println!("✓ Found {} resource(s)\n", resource_list.len());

                for resource in &resource_list {
                    println!("┌─────────────────────────────────────");
                    println!("│ 📚 {}", resource.uri);
                    println!("├─────────────────────────────────────");
                    println!("│ Name: {}", resource.name);
                    if let Some(ref mime_type) = resource.mime_type {
                        println!("│ Type: {}", mime_type);
                    }
                    println!("│ {}", resource.description);
                    println!("└─────────────────────────────────────\n");
                }

                if !resource_list.is_empty() {
                    println!("📖 Reading first resource...");
                    match client.read_resource(&resource_list[0].uri).await {
                        Ok(content) => {
                            println!("✓ Resource content:\n");
                            for part in &content.contents {
                                if let Some(ref text) = part.text {
                                    println!("{}\n", text);
                                } else {
                                    println!("⚠️  Non-text content (mime type: {})\n",
                                        part.mime_type.as_deref().unwrap_or("unknown"));
                                }
                            }
                        }
                        Err(e) => {
                            println!("✗ Failed to read resource: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                println!("✗ Failed to list resources: {}", e);
            }
        }
    }

    if capabilities.prompts {
        println!("\n🤖 Listing prompts...");
        let prompts = client.list_prompts().await;

        if !prompts.is_empty() {
            println!("✓ Found {} prompt(s)\n", prompts.len());

            for prompt in &prompts {
                println!("┌─────────────────────────────────────");
                println!("│ 💬 {}", prompt.name);
                println!("├─────────────────────────────────────");
                println!("│ {}", prompt.description);
                if let Some(ref arguments) = prompt.arguments {
                    if !arguments.is_empty() {
                        println!("│");
                        println!("│ Arguments:");
                        for arg in arguments {
                            println!("│   • {}: {}", arg.name, arg.description);
                            if let Some(ref required) = arg.required {
                                println!("│     Required: {}", required);
                            }
                        }
                    }
                }
                println!("└─────────────────────────────────────\n");
            }
        } else {
            println!("⚠️  No prompts available\n");
        }
    }

    Ok(())
}

/// Create MCP tool adapters and register them in ToolRegistry
async fn mcp_adapter(server: String) -> Result<()> {
    println!("🔌 Testing MCP Tool Adapter: {}\n", server);

    // Determine command and args
    let (command, args): (&str, Vec<&str>) = if server == "everything" {
        ("mcp-everything", vec![])
    } else if server == "tree-sitter" {
        ("tree-sitter", vec!["--stdio"])
    } else {
        (server.as_str(), vec![])
    };

    // Check if server is available
    println!("🔍 Checking if '{}' is in PATH...", command);
    match tokio::process::Command::new(command)
        .arg("--version")
        .output()
        .await
    {
        Ok(_) => println!("✓ Server found"),
        Err(_) => {
            println!("⚠️  Server '{}' not found in PATH", command);
            println!("   Skipping MCP adapter demo");
            return Ok(());
        }
    }

    // Spawn and initialize
    println!("\n🚀 Spawning MCP server...");
    let client = match McpClient::spawn(command, &args).await {
        Ok(c) => {
            println!("✓ Server spawned");
            c
        }
        Err(e) => {
            println!("✗ Failed to spawn: {}", e);
            return Ok(());
        }
    };

    println!("\n📡 Initializing...");
    match client.initialize().await {
        Ok(_) => println!("✓ Initialized"),
        Err(e) => {
            println!("✗ Failed: {}", e);
            return Ok(());
        }
    }

    // List tools
    println!("\n🔧 Listing tools...");
    let tools = match client.list_tools().await {
        Ok(t) => {
            println!("✓ Found {} tool(s)", t.len());
            t
        }
        Err(e) => {
            println!("✗ Failed: {}", e);
            return Ok(());
        }
    };

    if tools.is_empty() {
        println!("\n⚠️  No tools available from server");
        return Ok(());
    }

    // Create adapters and register in ToolRegistry
    println!("\n📦 Creating McpToolAdapter instances...\n");

    let mut registry = ToolRegistry::new();
    let client_arc = Arc::new(client);

    for tool_def in &tools {
        let adapter = McpToolAdapter::new(
            client_arc.clone(),
            tool_def.name.clone(),
            tool_def.description.clone(),
            tool_def.input_schema.clone(),
        );

        println!("✓ Created adapter: {}", adapter.name());
        println!("  Description: {}", adapter.description().short);

        // Register in ToolRegistry
        registry.register(Arc::new(adapter))?;
    }

    println!("\n✓ Registered {} MCP tool(s) in ToolRegistry", tools.len());

    // List all registered tools
    println!("\n📋 Tools in Registry:");
    for name in registry.list() {
        println!("   • {}", name);
    }

    // Show OpenAI format
    println!("\n📤 OpenAI Function Format:");
    let openai_format = registry.to_openai_format();
    println!("{} tools converted to OpenAI format", openai_format.len());
    
    if !openai_format.is_empty() {
        println!("\nExample (first tool):");
        println!("{}", serde_json::to_string_pretty(&openai_format[0])?);
    }

    Ok(())
}
