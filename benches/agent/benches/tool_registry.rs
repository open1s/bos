//! Benchmarks for tool registry operations
//!
//! These benchmarks measure the performance of tool registry operations,
//! focusing on the hot paths identified in the tool registry:
//! - O(n) lookup when searching across namespaces
//! - Tool registration overhead
//! - Listing and filtering operations

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use agent::tools::{Tool, ToolDescription};
use agent::error::ToolError;
use async_trait::async_trait;
use agent::tools::registry::ToolRegistry;
use std::sync::Arc;
use tokio::runtime::Runtime;

struct DummyTool {
    name: String,
}

impl DummyTool {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Tool for DummyTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: format!("A dummy tool called {}", self.name),
            parameters: "none".to_string(),
        }
    }

    fn json_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        Ok(serde_json::json!("executed"))
    }
}

/// Benchmark tool registration
///
/// Measures the cost of registering tools into the registry.
/// This is important for understanding startup time when loading skills.
fn bench_tool_registration(c: &mut Criterion) {
    let mut group = c.benchmark_group("tool_registration");
    group.measurement_time(std::time::Duration::from_secs(10));
    group.warm_up_time(std::time::Duration::from_secs(3));
    group.sample_size(200);

    // Benchmark single tool registration
    group.bench_function("single_tool", |b| {
        b.iter(|| {
            let mut registry = ToolRegistry::new();
            let tool = Arc::new(DummyTool::new("test_tool"));
            black_box(registry.register(tool)).unwrap();
        });
    });

    // Benchmark registering tools with namespace
    group.bench_function("with_namespace", |b| {
        b.iter(|| {
            let mut registry = ToolRegistry::new();
            let tool = Arc::new(DummyTool::new("test_tool"));
            black_box(registry.register_with_namespace(tool, "test_namespace")).unwrap();
        });
    });

    // Benchmark batch registration from skill
    for tool_count in [5, 10, 20, 50].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(tool_count), tool_count, |b, &count| {
            let tools: Vec<Arc<dyn Tool>> = (0..count)
                .map(|i| Arc::new(DummyTool::new(&format!("tool_{}", i))) as Arc<dyn Tool>)
                .collect();

            b.iter(|| {
                let mut registry = ToolRegistry::new();
                black_box(registry.register_from_skill("test_skill", tools.clone())).unwrap();
            });
        });
    }

    group.finish();
}

/// Benchmark tool lookup operations
///
/// Measures the cost of looking up tools by name.
/// The O(n) search across namespaces is a known hot path.
fn bench_tool_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("tool_lookup");
    group.measurement_time(std::time::Duration::from_secs(10));
    group.warm_up_time(std::time::Duration::from_secs(3));
    group.sample_size(200);

    // Benchmark exact match lookup
    group.bench_function("exact_match", |b| {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new("test_tool"))).unwrap();

        b.iter(|| {
            black_box(registry.get("test_tool"));
        });
    });

    // Benchmark lookup with namespace
    group.bench_function("with_namespace", |b| {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(DummyTool::new("test_tool"));
        registry.register_with_namespace(tool, "test_namespace").unwrap();

        b.iter(|| {
            black_box(registry.get_from_namespace("test_tool", "test_namespace"));
        });
    });

    // Benchmark O(n) search across namespaces
    for tool_count in [10, 50, 100, 200].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(tool_count), tool_count, |b, &count| {
            let mut registry = ToolRegistry::new();

            // Register tools in multiple namespaces
            for i in 0..count {
                let namespace = format!("namespace_{}", i % 5);
                let tool = Arc::new(DummyTool::new(&format!("tool_{}", i)));
                registry.register_with_namespace(tool, &namespace).unwrap();
            }

            // Lookup a tool that requires searching across namespaces
            b.iter(|| {
                black_box(registry.get("tool_0"));
            });
        });
    }

    group.finish();
}

/// Benchmark listing operations
///
/// Measures the cost of listing tools and namespaces.
fn bench_listing_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("listing_operations");
    group.measurement_time(std::time::Duration::from_secs(10));
    group.warm_up_time(std::time::Duration::from_secs(3));
    group.sample_size(200);

    // Benchmark list all tools
    for tool_count in [10, 50, 100, 200].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(tool_count), tool_count, |b, &count| {
            let mut registry = ToolRegistry::new();

            for i in 0..count {
                let tool = Arc::new(DummyTool::new(&format!("tool_{}", i)));
                registry.register(tool).unwrap();
            }

            b.iter(|| {
                black_box(registry.list());
            });
        });
    }

    // Benchmark list namespace
    for tool_count in [10, 50, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(tool_count), tool_count, |b, &count| {
            let mut registry = ToolRegistry::new();

            for i in 0..count {
                let namespace = format!("namespace_{}", i % 3);
                let tool = Arc::new(DummyTool::new(&format!("tool_{}", i)));
                registry.register_with_namespace(tool, &namespace).unwrap();
            }

            b.iter(|| {
                black_box(registry.list_namespace("namespace_0"));
            });
        });
    }

    // Benchmark list namespaces
    for tool_count in [10, 50, 100].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(tool_count), tool_count, |b, &count| {
            let mut registry = ToolRegistry::new();

            for i in 0..count {
                let namespace = format!("namespace_{}", i % 5);
                let tool = Arc::new(DummyTool::new(&format!("tool_{}", i)));
                registry.register_with_namespace(tool, &namespace).unwrap();
            }

            b.iter(|| {
                black_box(registry.list_namespaces());
            });
        });
    }

    group.finish();
}

/// Benchmark OpenAI format conversion
///
/// Measures the cost of converting all tools to OpenAI function format.
/// This is called when preparing tool definitions for LLM requests.
fn bench_openai_format(c: &mut Criterion) {
    let mut group = c.benchmark_group("openai_format");
    group.measurement_time(std::time::Duration::from_secs(10));
    group.warm_up_time(std::time::Duration::from_secs(3));
    group.sample_size(200);

    for tool_count in [5, 10, 20, 50].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(tool_count), tool_count, |b, &count| {
            let mut registry = ToolRegistry::new();

            for i in 0..count {
                let tool = Arc::new(DummyTool::new(&format!("tool_{}", i)));
                registry.register(tool).unwrap();
            }

            b.iter(|| {
                black_box(registry.to_openai_format());
            });
        });
    }

    group.finish();
}

/// Benchmark tool execution
///
/// Measures the overhead of tool execution through the registry.
fn bench_tool_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("tool_execution");
    group.measurement_time(std::time::Duration::from_secs(10));
    group.warm_up_time(std::time::Duration::from_secs(3));
    group.sample_size(200);

    let rt = Runtime::new().unwrap();

    // Benchmark execute with valid tool
    group.bench_function("valid_tool", |b| {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new("test_tool"))).unwrap();

        b.to_async(&rt).iter(|| async {
            let args = serde_json::json!({});
            let result = registry.execute("test_tool", &args).await;
            black_box(result.unwrap());
        });
    });

    // Benchmark execute with non-existent tool
    group.bench_function("not_found", |b| {
        let registry = ToolRegistry::new();

        b.to_async(&rt).iter(|| async {
            let args = serde_json::json!({});
            let result = registry.execute("nonexistent", &args).await;
            black_box(result);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_tool_registration,
    bench_tool_lookup,
    bench_listing_operations,
    bench_openai_format,
    bench_tool_execution
);
criterion_main!(benches);
