//! Author-facing extension trait and runtime adapter.

use std::path::PathBuf;

use ferris_core::{Contribution, ContributionRef, InvokeContext, Runtime, RuntimeContext};
use serde_json::Value;

use crate::error::{runtime as runtime_error, Result};
use crate::manifest::ExtensionManifest;

/// One invocation addressed to an extension.
#[derive(Debug, Clone)]
pub struct Call {
    /// Contribution being invoked.
    pub contribution: ContributionRef,
    /// Opaque input.
    pub input: Value,
}

impl Call {
    /// The contribution id.
    pub fn id(&self) -> &str {
        &self.contribution.id
    }

    /// The contribution kind.
    pub fn kind(&self) -> &str {
        &self.contribution.kind
    }
}

/// Context available while an extension starts.
#[derive(Debug, Clone)]
pub struct ExtensionContext {
    /// Framework runtime context.
    pub runtime: RuntimeContext,
    /// The validated manifest.
    pub manifest: ExtensionManifest,
}

/// Author-facing extension interface.
#[async_trait::async_trait]
pub trait Extension: Send + Sync {
    /// The extension manifest, schema 2.0.
    fn manifest(&self) -> ExtensionManifest;

    /// Start and return contributions.
    async fn start(&self, ctx: &ExtensionContext) -> Result<Vec<Contribution>>;

    /// Invoke one contribution.
    async fn invoke(&self, call: &Call, ctx: &InvokeContext) -> Result<Value>;

    /// Shut down.
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

/// Adapt an author Extension into the low-level Runtime interface.
pub struct ExtensionRuntime<E> {
    extension: E,
    kind: String,
    root: PathBuf,
    manifest: ExtensionManifest,
}

impl<E: Extension> ExtensionRuntime<E> {
    /// Wrap an extension, deriving the runtime kind from its manifest.
    pub fn new(extension: E, root: PathBuf) -> Result<Self> {
        let manifest = extension.manifest();
        let kind = manifest.runtime_kind().to_string();
        Ok(Self {
            extension,
            kind,
            root,
            manifest,
        })
    }

    /// The validated manifest.
    pub fn manifest(&self) -> &ExtensionManifest {
        &self.manifest
    }

    /// The package root.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Start the extension with a context built from its manifest.
    pub async fn start(&self) -> Result<Vec<Contribution>> {
        let ctx = RuntimeContext {
            extension: self.manifest.id().as_str().to_string(),
            root: self.root.clone(),
            data_root: self.root.join("data"),
        };
        <Self as Runtime>::start(self, &ctx).await
    }

    /// Invoke one contribution id.
    pub async fn invoke_id(&self, id: &str, input: Value) -> Result<Value> {
        <Self as Runtime>::invoke(
            self,
            &ContributionRef {
                kind: "tool".to_string(),
                id: id.to_string(),
            },
            input,
            &InvokeContext {
                correlation: "sdk-call".to_string(),
                timeout_ms: 30_000,
            },
        )
        .await
    }
}

#[async_trait::async_trait]
impl<E: Extension> Runtime for ExtensionRuntime<E> {
    fn kind(&self) -> &str {
        &self.kind
    }

    async fn start(&self, ctx: &RuntimeContext) -> Result<Vec<Contribution>> {
        if ctx.root != self.root {
            return Err(runtime_error(
                "extension runtime root does not match its context",
            ));
        }
        if ctx.extension != self.manifest.id().as_str() {
            return Err(runtime_error(
                "extension runtime id does not match its manifest",
            ));
        }
        self.extension
            .start(&ExtensionContext {
                runtime: ctx.clone(),
                manifest: self.manifest.clone(),
            })
            .await
    }

    async fn invoke(
        &self,
        contribution: &ContributionRef,
        input: Value,
        ctx: &InvokeContext,
    ) -> Result<Value> {
        self.extension
            .invoke(
                &Call {
                    contribution: contribution.clone(),
                    input,
                },
                ctx,
            )
            .await
    }

    async fn shutdown(&self) -> Result<()> {
        self.extension.shutdown().await
    }
}

/// Small in-process harness for an SDK extension.
pub struct TestHarness<E> {
    runtime: ExtensionRuntime<E>,
}

impl<E: Extension> TestHarness<E> {
    /// Build a harness around one extension and package root.
    pub fn new(extension: E, root: PathBuf) -> Result<Self> {
        Ok(Self {
            runtime: ExtensionRuntime::new(extension, root)?,
        })
    }

    /// The adapter under test.
    pub fn runtime(&self) -> &ExtensionRuntime<E> {
        &self.runtime
    }

    /// Start the extension.
    pub async fn start(&self) -> Result<Vec<Contribution>> {
        self.runtime.start().await
    }

    /// Invoke a tool by contribution id.
    pub async fn invoke(&self, id: &str, input: Value) -> Result<Value> {
        self.runtime.invoke_id(id, input).await
    }

    /// Shut the extension down.
    pub async fn shutdown(&self) -> Result<()> {
        <ExtensionRuntime<E> as Runtime>::shutdown(&self.runtime).await
    }
}
