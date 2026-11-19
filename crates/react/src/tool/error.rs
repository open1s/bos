use thiserror::Error;

/// A stable, machine-readable class for one tool failure.
///
/// The class is what a model and a host branch on; the `message` is what a
/// reader reads. They are separate fields on purpose, because a host that
/// decides "do not retry a permission failure" must not have to match prose,
/// and because prose is allowed to change without breaking that decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolErrorCode {
    /// The arguments did not satisfy the tool contract.
    InvalidInput,
    /// The tool, the resource, or a referenced entity does not exist.
    NotFound,
    /// A policy or an authority refused the call.
    PermissionDenied,
    /// The call contradicts the current state of the resource.
    Conflict,
    /// More than one target matches and the caller named none of them.
    Ambiguous,
    /// The call did not finish inside its bound.
    Timeout,
    /// The caller or the runtime stopped the call.
    Cancelled,
    /// Something the tool depends on is not available right now.
    Unavailable,
    /// The tool does not implement the requested operation.
    Unsupported,
    /// The tool ran and failed in a way it cannot classify further.
    Internal,
}

impl ToolErrorCode {
    /// The stable lower-case name used on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::NotFound => "not_found",
            Self::PermissionDenied => "permission_denied",
            Self::Conflict => "conflict",
            Self::Ambiguous => "ambiguous",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Unavailable => "unavailable",
            Self::Unsupported => "unsupported",
            Self::Internal => "internal",
        }
    }
}

impl std::fmt::Display for ToolErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Tool execution error.
///
/// This is the canonical tool error for the workspace. `agent` re-exports it
/// rather than defining its own, so a tool author only handles one type.
///
/// Every variant answers three questions the runtime needs: [`ToolError::code`]
/// (what class of failure), [`ToolError::retryable`] (may an unchanged retry
/// succeed), and [`ToolError::indeterminate`] (may the world have already
/// changed). A timeout and a cancellation are distinct variants because one is
/// a bound and the other is a request, and neither implies that a side effect
/// did not happen.
#[derive(Debug, Error, Clone)]
pub enum ToolError {
    /// No tool with that name is registered.
    #[error("Tool not found: {0}")]
    NotFound(String),
    /// The tool ran but failed.
    #[error("Tool execution failed: {0}")]
    Failed(String),
    /// More than one registered tool matches the bare name.
    #[error("Tool name {name} is ambiguous: {candidates:?}")]
    Ambiguous {
        /// The bare name the caller asked for.
        name: String,
        /// Sorted, fully qualified candidate names.
        candidates: Vec<String>,
    },
    /// The arguments were rejected.
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    /// The arguments did not match the tool schema.
    #[error("Schema mismatch: {message}")]
    SchemaMismatch {
        /// Description of the mismatch.
        message: String,
    },
    /// One argument field was missing or malformed.
    ///
    /// The field is carried as its own field, not woven into the message, so
    /// the caller can repair exactly that argument instead of re-reading the
    /// whole schema.
    #[error("Invalid input: field '{field}' {message}")]
    InvalidField {
        /// The offending argument name.
        field: String,
        /// What was wrong with it.
        message: String,
        /// The field was absent rather than present and malformed.
        missing: bool,
    },
    /// A policy or an authority refused the call.
    ///
    /// Authorization remains Runtime-controlled; this variant only reports the
    /// refusal it was handed.
    #[error("Permission denied: {message}")]
    PermissionDenied {
        /// Why the call was refused.
        message: String,
    },
    /// The call contradicts the current state of the resource.
    #[error("Tool conflict: {message}")]
    Conflict {
        /// What the call contradicted.
        message: String,
    },
    /// The call did not finish in time.
    ///
    /// Carries the timeout's own account of itself: which call exceeded which
    /// bound, and the output captured before the process was killed. A bare
    /// "timed out" leaves the reader unable to tell a one-second cap from the
    /// five-minute default, and throws away what the command had already said.
    #[error("Tool execution timed out: {0}")]
    Timeout(String),
    /// The caller or the runtime stopped the call.
    ///
    /// A cancellation is not a rollback: a tool that had already changed the
    /// world keeps those changes, so the outcome stays indeterminate.
    #[error("Tool execution cancelled: {0}")]
    Cancelled(String),
    /// Something the tool depends on is not available right now.
    #[error("Tool dependency unavailable: {message}")]
    Unavailable {
        /// What was unavailable.
        message: String,
    },
    /// The tool does not implement the requested operation.
    #[error("Unsupported operation: {message}")]
    Unsupported {
        /// What the tool cannot do.
        message: String,
    },
}

impl ToolError {
    /// The stable class of this failure.
    pub fn code(&self) -> ToolErrorCode {
        match self {
            Self::NotFound(_) => ToolErrorCode::NotFound,
            Self::Failed(_) => ToolErrorCode::Internal,
            Self::Ambiguous { .. } => ToolErrorCode::Ambiguous,
            Self::InvalidInput(_) | Self::SchemaMismatch { .. } | Self::InvalidField { .. } => {
                ToolErrorCode::InvalidInput
            }
            Self::PermissionDenied { .. } => ToolErrorCode::PermissionDenied,
            Self::Conflict { .. } => ToolErrorCode::Conflict,
            Self::Timeout(_) => ToolErrorCode::Timeout,
            Self::Cancelled(_) => ToolErrorCode::Cancelled,
            Self::Unavailable { .. } => ToolErrorCode::Unavailable,
            Self::Unsupported { .. } => ToolErrorCode::Unsupported,
        }
    }

    /// Whether an unchanged retry may succeed.
    ///
    /// This is an admission, not a promise: a caller must still respect the
    /// tool's declared idempotency. A timeout is deliberately NOT retryable,
    /// because the operation may have started; check the outcome first.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }

    /// Whether the world may already have changed despite the failure.
    ///
    /// True for a timeout, a cancellation, and any failure reported after the
    /// tool body ran: in all three the side effect may have happened and must
    /// be inspected before a retry.
    pub fn indeterminate(&self) -> bool {
        matches!(
            self,
            Self::Timeout(_) | Self::Cancelled(_) | Self::Failed(_)
        )
    }

    /// The offending argument name, when the failure names one.
    pub fn field(&self) -> Option<&str> {
        match self {
            Self::InvalidField { field, .. } => Some(field.as_str()),
            _ => None,
        }
    }

    /// Whether the named argument was absent rather than malformed.
    pub fn missing_field(&self) -> bool {
        matches!(self, Self::InvalidField { missing: true, .. })
    }

    /// A failure that names one absent argument.
    pub fn missing(field: impl Into<String>) -> Self {
        let field = field.into();
        Self::InvalidField {
            message: "is required".to_string(),
            field,
            missing: true,
        }
    }

    /// A failure that names one malformed argument.
    pub fn invalid_field(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidField {
            field: field.into(),
            message: message.into(),
            missing: false,
        }
    }

    /// A policy refusal.
    pub fn denied(message: impl Into<String>) -> Self {
        Self::PermissionDenied {
            message: message.into(),
        }
    }

    /// A cancellation that names what stopped.
    pub fn cancelled(reason: impl Into<String>) -> Self {
        Self::Cancelled(reason.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_classifies_without_inspecting_its_message() {
        let cases = [
            (ToolError::NotFound("x".into()), ToolErrorCode::NotFound),
            (ToolError::Failed("x".into()), ToolErrorCode::Internal),
            (
                ToolError::Ambiguous {
                    name: "x".into(),
                    candidates: vec!["a_x".into()],
                },
                ToolErrorCode::Ambiguous,
            ),
            (
                ToolError::InvalidInput("x".into()),
                ToolErrorCode::InvalidInput,
            ),
            (
                ToolError::SchemaMismatch {
                    message: "x".into(),
                },
                ToolErrorCode::InvalidInput,
            ),
            (ToolError::missing("path"), ToolErrorCode::InvalidInput),
            (
                ToolError::PermissionDenied {
                    message: "x".into(),
                },
                ToolErrorCode::PermissionDenied,
            ),
            (
                ToolError::Conflict {
                    message: "x".into(),
                },
                ToolErrorCode::Conflict,
            ),
            (ToolError::Timeout("x".into()), ToolErrorCode::Timeout),
            (ToolError::Cancelled("x".into()), ToolErrorCode::Cancelled),
            (
                ToolError::Unavailable {
                    message: "x".into(),
                },
                ToolErrorCode::Unavailable,
            ),
            (
                ToolError::Unsupported {
                    message: "x".into(),
                },
                ToolErrorCode::Unsupported,
            ),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code, "{error:?}");
        }
    }

    #[test]
    fn the_wire_names_are_the_documented_ones() {
        assert_eq!(ToolErrorCode::InvalidInput.as_str(), "invalid_input");
        assert_eq!(ToolErrorCode::NotFound.as_str(), "not_found");
        assert_eq!(
            ToolErrorCode::PermissionDenied.as_str(),
            "permission_denied"
        );
        assert_eq!(ToolErrorCode::Conflict.as_str(), "conflict");
        assert_eq!(ToolErrorCode::Ambiguous.as_str(), "ambiguous");
        assert_eq!(ToolErrorCode::Timeout.as_str(), "timeout");
        assert_eq!(ToolErrorCode::Cancelled.as_str(), "cancelled");
        assert_eq!(ToolErrorCode::Unavailable.as_str(), "unavailable");
        assert_eq!(ToolErrorCode::Unsupported.as_str(), "unsupported");
        assert_eq!(ToolErrorCode::Internal.as_str(), "internal");
    }

    /// A permission failure is never retryable; a timeout never claims the
    /// side effect did not happen; an unavailable dependency is the one class
    /// an unchanged retry may clear.
    #[test]
    fn retry_and_indeterminacy_are_not_the_same_question() {
        assert!(!ToolError::denied("no").retryable());
        assert!(!ToolError::denied("no").indeterminate());

        let timeout = ToolError::Timeout("120 s".into());
        assert!(!timeout.retryable(), "check the outcome before retrying");
        assert!(timeout.indeterminate(), "the operation may have started");

        let cancelled = ToolError::Cancelled("turn stopped".into());
        assert!(!cancelled.retryable());
        assert!(cancelled.indeterminate(), "cancellation is not rollback");
        assert_ne!(timeout.code(), cancelled.code());

        let unavailable = ToolError::Unavailable {
            message: "browser engine is not running".into(),
        };
        assert!(unavailable.retryable());
        assert!(!unavailable.indeterminate());
    }

    #[test]
    fn a_field_failure_names_exactly_one_argument() {
        let missing = ToolError::missing("path");
        assert_eq!(missing.field(), Some("path"));
        assert!(missing.missing_field());
        assert!(missing.to_string().contains("path"), "{missing}");

        let malformed = ToolError::invalid_field("status", "must be one of pending, done");
        assert_eq!(malformed.field(), Some("status"));
        assert!(!malformed.missing_field());
        assert_eq!(malformed.code(), ToolErrorCode::InvalidInput);

        assert_eq!(ToolError::NotFound("x".into()).field(), None);
    }
}
