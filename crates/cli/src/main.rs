use clap::{Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
#[command(name = "bos")]
#[command(about = "BOS - BrainOS CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, default_value = "json")]
    output: String,
}

#[derive(Subcommand)]
enum Commands {
    Run {
        prompt: String,
        #[arg(long, default_value = "gpt-4")]
        model: String,
        #[arg(long, default_value_t = 10)]
        max_steps: usize,
    },
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    List,
    Config,
}

#[derive(Subcommand)]
enum SessionAction {
    Create { name: Option<String> },
    Resume { id: String },
    Delete { id: String },
}

fn output_json(data: impl serde::Serialize) {
    println!("{}", serde_json::to_string_pretty(&data).unwrap());
}

#[allow(dead_code)]
fn output_text(data: impl serde::Serialize) {
    if let Ok(s) = serde_json::from_value::<String>(serde_json::to_value(&data).unwrap()) {
        println!("{}", s);
    } else {
        let json = serde_json::to_string_pretty(&data).unwrap();
        println!("{}", json);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { prompt, model, max_steps } => {
            output_json(json!({
                "status": "ok",
                "message": "Run command not fully implemented yet",
                "prompt": prompt,
                "model": model,
                "max_steps": max_steps
            }));
        }
        Commands::Session { action } => {
            match action {
                SessionAction::Create { name } => {
                    output_json(json!({
                        "status": "created",
                        "session_id": "new-session-id",
                        "name": name
                    }));
                }
                SessionAction::Resume { id } => {
                    output_json(json!({
                        "status": "resumed",
                        "session_id": id
                    }));
                }
                SessionAction::Delete { id } => {
                    output_json(json!({
                        "status": "deleted",
                        "session_id": id
                    }));
                }
            }
        }
        Commands::List => {
            output_json(json!({
                "status": "ok",
                "sessions": []
            }));
        }
        Commands::Config => {
            output_json(json!({
                "status": "ok",
                "config": {
                    "llm": {
                        "provider": "openai",
                        "model": "gpt-4"
                    }
                }
            }));
        }
    }

    Ok(())
}