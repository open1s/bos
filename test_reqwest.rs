use std::thread;
use std::time::Duration;

#[tokio::main]
async fn main() {
    println!("[TEST] Testing reqwest client...");
    
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap();
    
    let body = serde_json::json!({"test": "data"});
    
    match client.post("http://127.0.0.1:9898/test")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await {
        Ok(resp) => {
            println!("[TEST] Status: {}", resp.status());
            match resp.text().await {
                Ok(text) => println!("[TEST] Body: {}", text),
                Err(e) => println!("[TEST] Body error: {}", e),
            }
        }
        Err(e) => println!("[TEST] Request error: {}", e),
    }
}
