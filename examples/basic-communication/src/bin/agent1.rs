use std::io::{self, Write};
use agent::a2a::{AgentIdentity, A2ADiscovery, AgentCard, A2AClient, Task};
use brainos_common::{setup_bus, setup_logging};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_logging()?;

    println!("╔══════════════════════════════════════╗");
    println!("║  Agent 1 (Bob) - A2A Communication  ║");
    println!("╚══════════════════════════════════════╝\n");

    let session = setup_bus(None).await?;

    let identity = AgentIdentity::new(
        "agent-1".to_string(),
        "Bob".to_string(),
        "0.1.0".to_string(),
    );

    tracing::info!("Created identity for Bob (agent-1)");

    let discovery = A2ADiscovery::new(session.clone());
    let card = AgentCard::new(
        identity.clone(),
        "Bob".to_string(),
        "A helpful agent who sends tasks".to_string(),
    )
    .with_capability("messaging".to_string(), "Send messages to other agents".to_string());

    discovery.announce(&card).await?;
    println!("✓ Announced as Bob (agent-1)\n");

    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    let agents = discovery.discover(None).await?;
    println!("✓ Discovered {} agent(s):\n", agents.len());

    let mut recipient: Option<AgentIdentity> = None;
    for agent_card in &agents {
        println!("  - {} ({})", agent_card.name, agent_card.agent_id.id);
        if agent_card.agent_id.id == "agent-2" {
            recipient = Some(agent_card.agent_id.clone());
        }
    }

    if recipient.is_none() && !agents.is_empty() {
        recipient = Some(agents[0].agent_id.clone());
    }

    match recipient {
        Some(ref rec) => println!("\n✓ Will send tasks to: {} ({})", rec.name, rec.id),
        None => println!("\n⚠️  No agents found. Start agent2 in another terminal."),
    }

    println!("\n{}", "=".repeat(50));
    println!("Interactive Mode - Enter messages to send to agents");
    println!("{}", "=".repeat(50));
    println!();

    let client = A2AClient::new(session.clone(), identity.clone());

    loop {
        print!("You> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        if input == "quit" || input == "exit" {
            println!("Goodbye!");
            break;
        }

        if recipient.is_none() {
            println!("⚠️  No recipient available. Waiting for agents...");
            let agents = discovery.discover(None).await?;
            for agent_card in &agents {
                if agent_card.agent_id.id == "agent-2" {
                    recipient = Some(agent_card.agent_id.clone());
                    break;
                }
            }
            if recipient.is_none() && !agents.is_empty() {
                recipient = Some(agents[0].agent_id.clone());
            }
            match recipient {
                Some(ref rec) => println!("✓ Found recipient: {} ({})", rec.name, rec.id),
                None => {
                    println!("⚠️  Still no agents found. Try again.");
                    continue;
                }
            }
        }

        println!("\nSending task to {}...", recipient.as_ref().unwrap().name);

        let task_id = uuid::Uuid::new_v4().to_string();
        let task = Task::new(task_id, serde_json::json!(input));

        match client.delegate_task(recipient.as_ref().unwrap(), task).await {
            Ok(task) => {
                let output = task.output.as_ref().and_then(|v| v.as_str()).unwrap_or("no output");
                println!("✓ Response: {}\n", output);
            }
            Err(e) => {
                println!("✗ Failed to send task: {}\n", e);
            }
        }
    }

    Ok(())
}