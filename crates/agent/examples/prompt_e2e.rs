use agent::mcp::McpClient;

#[tokio::main]
async fn main() {
    let client = McpClient::connect_http("http://127.0.0.1:8791/mcp");
    let caps = client.initialize().await.expect("initialize");
    println!(
        "caps ok: prompts={:?}",
        !caps.prompts.is_boolean() || caps.prompts.as_bool() != Some(false)
    );
    let prompts = client.list_prompts().await;
    println!(
        "prompts: {:?}",
        prompts.iter().map(|p| p.name.clone()).collect::<Vec<_>>()
    );
    let got = client
        .get_prompt(
            "review",
            Some(serde_json::json!({"focus": "error handling"})),
        )
        .await
        .expect("get_prompt");
    println!("description: {:?}", got.description);
    assert_eq!(got.messages.len(), 1);
    let text = got.messages[0]
        .content
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(text.contains("error handling"), "{text}");
    println!("fetched text: {text}");
    let err = client.get_prompt("nope", None).await;
    assert!(err.is_err(), "unknown prompt must error");
    println!("unknown prompt errors as expected");
    println!("E2E OK");
}
