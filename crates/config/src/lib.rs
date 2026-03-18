mod error;
pub mod loader;
pub mod types;

pub use error::{ConfigError, ConfigResult};
pub use loader::ConfigLoader;
pub use types::{ConfigFormat, ConfigMergeStrategy};
