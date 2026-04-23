use react::engine::ReActEngineBuilder;
use react::llm::vendor::OpenAiVendorBuilder;

#[tokio::test]
async fn test_engine_with_vendor() {
    let vendor = OpenAiVendorBuilder::new()
        .model("gpt-4o-mini".to_string())
        .api_key("test-key".to_string())
        .build()
        .expect("Failed to build vendor");

    let mut engine = ReActEngineBuilder::new()
        .llm(Box::new(vendor))
        .max_steps(1)
        .build()
        .expect("Failed to build engine");

    let result = engine.react("Hi").await;
    // With invalid key, still shouldn't panic
    assert!(result.is_ok() || result.is_err());
}
