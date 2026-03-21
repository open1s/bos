//! Benchmarks for LLM client operations
//!
//! These benchmarks measure the performance of LLM client operations,
//! focusing on the hot paths identified in the LLM client:
//! - JSON parsing per token in streaming responses
//! - Request building overhead
//! - Token parsing from SSE events

use agent::llm::{LlmRequest, OpenAiMessage, StreamToken};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::time::Duration;

/// Benchmark JSON parsing for streaming tokens
///
/// Measures the cost of parsing JSON from SSE events.
/// This is a hot path that happens for every token in streaming responses.
fn bench_json_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_parsing");
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));
    group.sample_size(200);

    // Benchmark parsing text token JSON
    group.bench_function("text_token", |b| {
        let json = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
        b.iter(|| {
            let _parsed: serde_json::Value = serde_json::from_str(black_box(json)).unwrap();
        });
    });

    // Benchmark parsing tool call JSON
    group.bench_function("tool_call_token", |b| {
        let json = r#"{"choices":[{"delta":{"tool_calls":[{"function":{"name":"test_tool","arguments":"{\"param\":\"value\"}"}}]}}]}"#;
        b.iter(|| {
            let _parsed: serde_json::Value = serde_json::from_str(black_box(json)).unwrap();
        });
    });

    // Benchmark parsing empty delta
    group.bench_function("empty_delta", |b| {
        let json = r#"{"choices":[{"delta":{}}]}"#;
        b.iter(|| {
            let _parsed: serde_json::Value = serde_json::from_str(black_box(json)).unwrap();
        });
    });

    group.finish();
}

/// Benchmark request building
///
/// Measures the cost of building LLM requests from internal types.
/// This happens before every LLM API call.
fn bench_request_building(c: &mut Criterion) {
    let mut group = c.benchmark_group("request_building");
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));
    group.sample_size(200);

    // Benchmark building simple request
    group.bench_function("simple_request", |b| {
        let req = LlmRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAiMessage::System {
                    content: "You are a helpful assistant.".to_string(),
                },
                OpenAiMessage::User {
                    content: "Hello!".to_string(),
                },
            ],
            tools: None,
            temperature: 0.7,
            max_tokens: Some(100),
        };

        b.iter(|| {
            let _json = serde_json::to_string(black_box(&req)).unwrap();
        });
    });

    // Benchmark building request with tools
    group.bench_function("with_tools", |b| {
        let tools = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "test_tool",
                "description": "A test tool",
                "parameters": {"type": "object", "properties": {}}
            }
        })];

        let req = LlmRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAiMessage::System {
                    content: "You are a helpful assistant.".to_string(),
                },
                OpenAiMessage::User {
                    content: "Use the test tool.".to_string(),
                },
            ],
            tools: Some(tools),
            temperature: 0.7,
            max_tokens: Some(100),
        };

        b.iter(|| {
            let _json = serde_json::to_string(black_box(&req)).unwrap();
        });
    });

    // Benchmark building request with conversation history
    for message_count in [5, 10, 20, 50].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(message_count),
            message_count,
            |b, &count| {
                let mut messages = vec![OpenAiMessage::System {
                    content: "You are a helpful assistant.".to_string(),
                }];

                for i in 0..count {
                    if i % 2 == 0 {
                        messages.push(OpenAiMessage::User {
                            content: format!("Message {}", i),
                        });
                    } else {
                        messages.push(OpenAiMessage::Assistant {
                            content: format!("Response {}", i),
                        });
                    }
                }

                let req = LlmRequest {
                    model: "gpt-4".to_string(),
                    messages,
                    tools: None,
                    temperature: 0.7,
                    max_tokens: Some(100),
                };

                b.iter(|| {
                    let _json = serde_json::to_string(black_box(&req)).unwrap();
                });
            },
        );
    }

    group.finish();
}

/// Benchmark token parsing from SSE events
///
/// Measures the cost of parsing StreamToken from JSON strings.
/// This is the core hot path in streaming responses.
fn bench_token_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("token_parsing");
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));
    group.sample_size(200);

    // Benchmark parsing text token
    group.bench_function("text_token", |b| {
        let json = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
        b.iter(|| {
            let data: serde_json::Value = serde_json::from_str(black_box(json)).unwrap();
            let content = data["choices"][0]["delta"]["content"].as_str().unwrap();
            let _token = StreamToken::Text(content.to_string());
        });
    });

    // Benchmark parsing tool call token
    group.bench_function("tool_call_token", |b| {
        let json = r#"{"choices":[{"delta":{"tool_calls":[{"function":{"name":"test_tool","arguments":"{\"param\":\"value\"}"}}]}}]}"#;
        b.iter(|| {
            let data: serde_json::Value = serde_json::from_str(black_box(json)).unwrap();
            if let Some(calls) = data["choices"][0]["delta"]["tool_calls"].as_array() {
                if let Some(call) = calls.first() {
                    let name = call["function"]["name"].as_str().unwrap();
                    let args_str = call["function"]["arguments"].as_str().unwrap();
                    let args: serde_json::Value = serde_json::from_str(args_str).unwrap();
                    let _token = StreamToken::ToolCall {
                        name: name.to_string(),
                        args,
                    };
                }
            }
        });
    });

    // Benchmark parsing empty token
    group.bench_function("empty_token", |b| {
        let json = r#"{"choices":[{"delta":{}}]}"#;
        b.iter(|| {
            let data: serde_json::Value = serde_json::from_str(black_box(json)).unwrap();
            let content = data["choices"][0]["delta"]["content"].as_str();
            let _is_empty = content.map_or(true, |s| s.is_empty());
        });
    });

    group.finish();
}

/// Benchmark message serialization
///
/// Measures the cost of converting OpenAiMessage to JSON format.
fn bench_message_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_serialization");
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));
    group.sample_size(200);

    // Benchmark system message
    group.bench_function("system_message", |b| {
        let msg = OpenAiMessage::System {
            content: "You are a helpful assistant.".to_string(),
        };
        b.iter(|| {
            let _json = serde_json::to_string(black_box(&msg)).unwrap();
        });
    });

    // Benchmark user message
    group.bench_function("user_message", |b| {
        let msg = OpenAiMessage::User {
            content: "Hello, how are you?".to_string(),
        };
        b.iter(|| {
            let _json = serde_json::to_string(black_box(&msg)).unwrap();
        });
    });

    // Benchmark assistant message
    group.bench_function("assistant_message", |b| {
        let msg = OpenAiMessage::Assistant {
            content: "I'm doing well, thank you!".to_string(),
        };
        b.iter(|| {
            let _json = serde_json::to_string(black_box(&msg)).unwrap();
        });
    });

    // Benchmark tool result message
    group.bench_function("tool_result_message", |b| {
        let msg = OpenAiMessage::ToolResult {
            tool_call_id: "call_123".to_string(),
            content: serde_json::json!({"result": "success"}).to_string(),
        };
        b.iter(|| {
            let _json = serde_json::to_string(black_box(&msg)).unwrap();
        });
    });

    group.finish();
}

/// Benchmark tool definition serialization
///
/// Measures the cost of serializing tool definitions to OpenAI format.
fn bench_tool_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("tool_serialization");
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(3));
    group.sample_size(200);

    // Benchmark simple tool
    group.bench_function("simple_tool", |b| {
        let tool = serde_json::json!({
            "type": "function",
            "function": {
                "name": "test_tool",
                "description": "A test tool",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        });
        b.iter(|| {
            let _json = serde_json::to_string(black_box(&tool)).unwrap();
        });
    });

    // Benchmark tool with parameters
    group.bench_function("with_parameters", |b| {
        let tool = serde_json::json!({
            "type": "function",
            "function": {
                "name": "calculator",
                "description": "Perform calculations",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "operation": {"type": "string", "enum": ["add", "subtract", "multiply", "divide"]},
                        "a": {"type": "number"},
                        "b": {"type": "number"}
                    },
                    "required": ["operation", "a", "b"]
                }
            }
        });
        b.iter(|| {
            let _json = serde_json::to_string(black_box(&tool)).unwrap();
        });
    });

    // Benchmark multiple tools
    for tool_count in [5, 10, 20].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(tool_count),
            tool_count,
            |b, &count| {
                let tools: Vec<serde_json::Value> = (0..count)
                    .map(|i| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": format!("tool_{}", i),
                                "description": format!("Tool number {}", i),
                                "parameters": {
                                    "type": "object",
                                    "properties": {
                                        "param": {"type": "string"}
                                    }
                                }
                            }
                        })
                    })
                    .collect();

                b.iter(|| {
                    let _json = serde_json::to_string(black_box(&tools)).unwrap();
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_json_parsing,
    bench_request_building,
    bench_token_parsing,
    bench_message_serialization,
    bench_tool_serialization
);
criterion_main!(benches);
