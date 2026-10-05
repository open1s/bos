//! The `ReActApp` hook trait and its no-op implementation.

/// The hook trait implementations.
pub mod app;
pub use app::{HookDecision, NoopApp, ReActApp};
