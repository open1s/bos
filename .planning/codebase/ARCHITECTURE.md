# ARCHITECTURE.md

## Overview

BrickOS (BrainOS) is a **distributed message bus** architecture with two core libraries:

1. **`bus`** - Zenoh-based pub/sub messaging
2. **`config`** - Multi-format configuration loading

## Design Pattern

### Message Bus Pattern

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  Publisher  │────▶│   Zenoh     │────▶│ Subscriber  │
└─────────────┘     │   Network  │     └─────────────┘
                     └─────────────┘
                           ▲
                           │
                     ┌─────┴─────┐
                     │  Queryable │
                     └───────────┘
```

### Configuration Pattern

```
┌─────────────────┐
│ ConfigLoader    │
├─────────────────┤
│ + add_file()    │
│ + add_directory()│
│ + add_inline()   │
│ + load()         │
├─────────────────┤
│ Strategy:        │
│ - Override       │
│ - DeepMerge      │
│ - First          │
│ - Accumulate     │
└─────────────────┘
```

## Crate Architecture

### `bus` Crate

```
bus/
├── lib.rs          # Public exports, ZenohConfig, JsonCodec
├── error.rs        # ZenohError enum
├── session.rs      # SessionManager, SessionManagerBuilder
├── publisher.rs    # PublisherWrapper (cached declaration)
├── subscriber.rs   # SubscriberWrapper<T> (generic)
├── query.rs        # QueryWrapper, QueryableWrapper
├── queryable.rs    # Queryable implementation
└── python.rs       # PyO3 bindings (feature-gated)
```

### `config` Crate

```
config/
├── lib.rs          # Public exports
├── error.rs        # ConfigError, ConfigResult
├── loader.rs       # ConfigLoader (1000+ lines)
├── types.rs        # ConfigFormat, ConfigSource, ConfigMergeStrategy
└── python.rs       # PyO3 bindings (feature-gated)
```

## Key Abstractions

### PublisherWrapper

Caches Zenoh publisher declaration for performance:

```rust
pub struct PublisherWrapper {
    topic: String,
    codec: JsonCodec,
    session: Option<Arc<Session>>,
    publisher: Option<CachedPublisher>,  // Cached!
}
```

### SubscriberWrapper<T>

Generic subscriber with automatic deserialization:

```rust
pub struct SubscriberWrapper<T: DeserializeOwned + Send + Sized + 'static> {
    topic: String,
    subscriber: Option<zenoh::pubsub::Subscriber<...>>,
    _phantom: PhantomData<T>,
}
```

### ConfigLoader

Builder pattern with fluent API:

```rust
let config = ConfigLoader::new()
    .with_strategy(ConfigMergeStrategy::DeepMerge)
    .add_file("base.toml")
    .add_directory("./config.d")
    .add_inline(serde_json::json!({"env": "prod"}))
    .load()
    .await?;
```

## Data Flow

### Publishing Messages

```
User Code → PublisherWrapper::new(topic) 
         → PublisherWrapper::init(session) 
         → session.declare_publisher(topic) [cached]
         → publisher.put(data)
```

### Subscribing to Messages

```
Zenoh Network → SubscriberWrapper::init(session)
            → session.declare_subscriber(topic)
            → subscriber.recv_async()
            → serde_json::from_slice()
            → User receives typed T
```

### Configuration Loading

```
ConfigLoader::load()
         → Match strategy
         → For each source:
           → ConfigSource::File → load_file()
           → ConfigSource::Directory → load_directory()
           → ConfigSource::Inline → use directly
           → ConfigSource::Custom → custom.load()
         → Apply merge strategy
         → Return cached Value
```

## Async/Await Patterns

- All I/O operations are async (tokio)
- Both sync and async config loading supported
- Session management with Arc<RwLock<Option<Arc<Session>>>>
- Graceful shutdown with timeout support
