//! The native provider: registered tools exposed through the capability contract.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ::capability::{
    CapabilityCall, CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityOutput,
    CapabilityProvider, InvocationId, ProviderHealth, ProviderId, ProviderKind,
};
use async_trait::async_trait;
use react::tool::ToolError;
use serde_json::Value;

use super::catalogue::catalogue_from_registry;
use crate::tools::{validate_args, ToolRegistry};

/// Registered tools exposed as one built-in capability provider.
pub struct NativeToolProvider {
    provider: ProviderId,
    registry: Arc<ToolRegistry>,
    in_flight: Mutex<HashMap<String, String>>,
}

impl NativeToolProvider {
    /// Build a provider over a registry this provider keeps alive.
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            provider: ProviderId::builtin(),
            registry,
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// Build a provider over a snapshot of a registry.
    pub fn from_registry(registry: &ToolRegistry) -> Self {
        Self::new(Arc::new(registry.clone()))
    }

    /// The registry this provider executes against.
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    fn remember(&self, invocation: &str, name: &str) {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        in_flight.insert(invocation.to_string(), name.to_string());
    }

    fn forget(&self, invocation: &str) {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        in_flight.remove(invocation);
    }

    fn in_flight_tool(&self, invocation: &str) -> Option<String> {
        self.in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(invocation)
            .cloned()
    }
}

#[async_trait]
impl CapabilityProvider for NativeToolProvider {
    fn id(&self) -> &ProviderId {
        &self.provider
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        let catalogue = catalogue_from_registry(&self.registry)?;
        Ok(catalogue.iter().cloned().collect())
    }

    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        let capability = call.capability.clone();
        let name = capability.name.clone();
        let started = Instant::now();

        if call.context.cancelled() {
            return Err(CapabilityError::Cancelled);
        }
        if call.context.expired(started) {
            return Err(CapabilityError::Timeout {
                elapsed_ms: 0,
                partial: None,
            });
        }

        // Resolve the surface before validating, so an unknown capability is a
        // NotFound rather than a schema complaint about a tool that does not
        // exist.
        let sync_tool = self.registry.get(&name);
        let async_tool = if sync_tool.is_none() {
            self.registry.get_async(&name)
        } else {
            None
        };
        let Some(schema) = sync_tool
            .as_ref()
            .map(|tool| tool.json_schema())
            .or_else(|| async_tool.as_ref().map(|tool| tool.json_schema()))
        else {
            return Err(CapabilityError::NotFound {
                capability: capability.clone(),
            });
        };

        validate_args(&schema, &call.input).map_err(|error| invalid(&error))?;

        let invocation = call.context.invocation.as_str().to_string();
        self.remember(&invocation, &name);

        let outcome = if let Some(tool) = sync_tool {
            // A synchronous body must not occupy an async worker: it may block
            // for as long as a process takes. It runs on the blocking pool,
            // and a deadline that expires first is reported as a timeout.
            let body = Arc::clone(&tool);
            let input = call.input.clone();
            let join = tokio::task::spawn_blocking(move || body.run(&input));
            match call.context.deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    match tokio::time::timeout(remaining, join).await {
                        Ok(Ok(result)) => {
                            result.map_err(|error| classify(&capability, error, started))
                        }
                        Ok(Err(error)) => Err(CapabilityError::ProviderFailed {
                            message: error.to_string(),
                        }),
                        Err(_) => Err(CapabilityError::Timeout {
                            elapsed_ms: started.elapsed().as_millis() as u64,
                            partial: None,
                        }),
                    }
                }
                None => match join.await {
                    Ok(result) => result.map_err(|error| classify(&capability, error, started)),
                    Err(error) => Err(CapabilityError::ProviderFailed {
                        message: error.to_string(),
                    }),
                },
            }
        } else if let Some(tool) = async_tool {
            let future = tool.run_with_call_id(&call.input, &invocation);
            match call.context.deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    match tokio::time::timeout(remaining, future).await {
                        Ok(result) => result.map_err(|error| classify(&capability, error, started)),
                        Err(_) => Err(CapabilityError::Timeout {
                            elapsed_ms: started.elapsed().as_millis() as u64,
                            partial: None,
                        }),
                    }
                }
                None => future
                    .await
                    .map_err(|error| classify(&capability, error, started)),
            }
        } else {
            Err(CapabilityError::NotFound {
                capability: capability.clone(),
            })
        };

        self.forget(&invocation);
        let value = outcome?;

        // A call that was cancelled while it ran still produced a value; the
        // caller asked for it to stop, so report the stop rather than a result
        // it will discard.
        if call.context.cancelled() {
            return Err(CapabilityError::Cancelled);
        }

        if call.context.expired(Instant::now()) {
            return Err(CapabilityError::Timeout {
                elapsed_ms: started.elapsed().as_millis() as u64,
                partial: Some(value),
            });
        }

        Ok(CapabilityOutput::json(value))
    }

    async fn cancel(&self, invocation: &InvocationId) -> Result<(), CapabilityError> {
        if let Some(name) = self.in_flight_tool(invocation.as_str()) {
            if let Some(tool) = self.registry.get_async(&name) {
                tool.cancel(invocation.as_str());
            }
        }
        Ok(())
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

fn invalid(error: &ToolError) -> CapabilityError {
    match error {
        ToolError::SchemaMismatch { message } => CapabilityError::Invalid {
            message: message.clone(),
        },
        other => CapabilityError::Invalid {
            message: other.to_string(),
        },
    }
}

fn classify(capability: &CapabilityId, error: ToolError, started: Instant) -> CapabilityError {
    match error {
        ToolError::NotFound(_) => CapabilityError::NotFound {
            capability: capability.clone(),
        },
        ToolError::InvalidInput(message) | ToolError::SchemaMismatch { message } => {
            CapabilityError::Invalid { message }
        }
        ToolError::Timeout(message) => CapabilityError::Timeout {
            elapsed_ms: started.elapsed().as_millis() as u64,
            partial: Some(Value::String(message)),
        },
        ToolError::Failed(message) => CapabilityError::ProviderFailed { message },
        ToolError::Ambiguous { name, candidates } => CapabilityError::Invalid {
            message: format!("tool {name} is ambiguous: {candidates:?}"),
        },
    }
}
