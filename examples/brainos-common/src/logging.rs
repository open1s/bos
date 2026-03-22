use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Setup structured logging for examples
///
/// # Example
///
/// ```rust
/// use brainos_common::setup_logging;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     setup_logging()?;
///     // Your code here
///     Ok(())
/// }
/// ```
pub fn setup_logging() -> anyhow::Result<()> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_target(true).with_thread_ids(true))
        .init();

    Ok(())
}
