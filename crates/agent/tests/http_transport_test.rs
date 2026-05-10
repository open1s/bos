//! HTTP Transport test for MCP server

use reqwest::Client;
use serde_json::json;
use std::time::Duration;

fn start_server(port: u16) -> std::process::Child {
    let cargo_manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let examples_path = std::path::Path::new(&cargo_manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates/examples");
    
    std::process::Command::new("python3")
        .current_dir(examples_path.as_os_str())
        .args(&[
            "-c",
            &format!(
                r#"
import sys
sys.path.insert(0, '.')
from mcp_http_server import run_server
run_server({})
"#,
                port
            ),
        ])
        .spawn()
        .unwrap()
}

#[tokio::test]
async fn test_http_mcp_with_ureq() {
    // Use ureq - a simpler synchronous HTTP/1.1 client
    let mut server = start_server(8783);
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    let base_url = "http://127.0.0.1:8783/mcp";
    println!("=== Testing with ureq (sync HTTP/1.1) at {} ===", base_url);
    
    // Test 1: initialize
    println!("\n1. Testing initialize with ureq...");
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "ureq-test", "version": "1.0"}
        }
    });
    
    let body_str = serde_json::to_string(&body).unwrap();
    match ureq::post(base_url)
        .header("Content-Type", "application/json")
        .send(body_str.as_bytes())
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            println!("Status: {}", status);
            let body = resp.into_body().read_to_string().unwrap_or_default();
            println!("Body: {:?}", body);
            assert_eq!(status, 200, "ureq initialize should return 200");
        }
        Err(e) => {
            panic!("ureq initialize failed: {}", e);
        }
    }

    // Test 2: tools/list
    println!("\n2. Testing tools/list with ureq...");
    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let body_str2 = serde_json::to_string(&body).unwrap();
    let resp = ureq::post(base_url)
        .header("Content-Type", "application/json")
        .send(body_str2.as_bytes())
        .unwrap();
    println!("Status: {}", resp.status().as_u16());
    assert_eq!(resp.status().as_u16(), 200, "ureq tools/list should return 200");

    // Test 3: tools/call
    println!("\n3. Testing tools/call with ureq...");
    let body = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "greet",
            "arguments": {"name": "BrainOS"}
        }
    });
    let body_str3 = serde_json::to_string(&body).unwrap();
    let resp = ureq::post(base_url)
        .header("Content-Type", "application/json")
        .send(body_str3.as_bytes())
        .unwrap();
    println!("Status: {}", resp.status().as_u16());
    assert_eq!(resp.status().as_u16(), 200, "ureq tools/call should return 200");

    let body = resp.into_body().read_to_string().unwrap();
    let resp_json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let content = resp_json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(content.contains("BrainOS"), "Response should contain 'BrainOS'");

    server.kill().ok();
    println!("\n✅ ureq tests passed!");
}

#[tokio::test]
async fn test_http_mcp_with_reqwest_no_proxy() {
    let mut server = start_server(8784);
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    let base_url = "http://127.0.0.1:8784/mcp";
    println!("=== Testing with reqwest (no_proxy) at {} ===", base_url);
    
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .http1_only()
        .no_proxy()
        .build()
        .unwrap();

    // Test initialize
    println!("\n1. Testing initialize with reqwest no_proxy...");
    let resp = client
        .post(base_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "reqwest-no-proxy", "version": "1.0"}
            }
        }))
        .send()
        .await
        .unwrap();
    
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    println!("Status: {}", status);
    println!("Body: {:?}", body);
    
    assert_eq!(status, 200, "reqwest no_proxy initialize should return 200");

    // Test tools/list
    println!("\n2. Testing tools/list with reqwest no_proxy...");
    let resp = client
        .post(base_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .unwrap();
    
    let status = resp.status();
    println!("Status: {}", status);
    assert_eq!(status, 200, "reqwest no_proxy tools/list should return 200");

    // Test tools/call
    println!("\n3. Testing tools/call with reqwest no_proxy...");
    let resp = client
        .post(base_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "greet",
                "arguments": {"name": "BrainOS"}
            }
        }))
        .send()
        .await
        .unwrap();
    
    let status = resp.status();
    let body = resp.text().await.unwrap();
    println!("Status: {}", status);
    assert_eq!(status, 200, "reqwest no_proxy tools/call should return 200");
    
    let resp_json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let content = resp_json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(content.contains("BrainOS"), "Response should contain 'BrainOS'");

    server.kill().ok();
    println!("\n✅ reqwest no_proxy tests passed!");
}

#[tokio::test]
async fn test_http_mcp_server() -> Result<(), Box<dyn std::error::Error>> {
    let mut server = start_server(8781);

    // Wait for server to start
    tokio::time::sleep(Duration::from_secs(2)).await;

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .http1_only()
        .build()?;

    let base_url = "http://127.0.0.1:8781/mcp";
    println!("Testing MCP HTTP server at {}", base_url);

    // Test 1: initialize
    println!("\n1. Testing initialize...");
    let resp = client
        .post(base_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"}
            }
        }))
        .send()
        .await?;
    
    let status = resp.status();
    println!("Status: {}", status);
    let body = resp.text().await?;
    println!("Body: {:?}", body);
    
    assert_eq!(status, 200, "initialize should return 200, got {}", status);

    // Test 2: tools/list
    println!("\n2. Testing tools/list...");
    let resp = client
        .post(base_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await?;
    
    let status = resp.status();
    println!("Status: {}", status);
    let body = resp.text().await?;
    println!("Body: {:?}", body);
    
    assert_eq!(status, 200, "tools/list should return 200");

    // Test 3: tools/call
    println!("\n3. Testing tools/call...");
    let resp = client
        .post(base_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "greet",
                "arguments": {"name": "BrainOS"}
            }
        }))
        .send()
        .await?;
    
    let status = resp.status();
    println!("Status: {}", status);
    let body = resp.text().await?;
    println!("Body: {:?}", body);
    
    assert_eq!(status, 200, "tools/call should return 200");
    
    // Verify response contains greeting
    let resp_json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let content = resp_json["result"]["content"][0]["text"].as_str().unwrap();
    assert!(content.contains("BrainOS"), "Response should contain 'BrainOS'");

    // Cleanup
    server.kill()?;
    
    println!("\n✅ All HTTP MCP tests passed!");
    Ok(())
}
