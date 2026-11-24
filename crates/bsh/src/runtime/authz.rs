//! Host-side authorization seam (RFC-019).
//!
//! Authorization is evaluated in the Host, never in a client and never from a
//! command payload. Stage 1 models one local OS principal; the same seam is
//! where remote principals, devices and workspace membership will be checked.

use super::contract::{PrincipalKind, PrincipalView, Rejection, RejectionCode, Target};
use super::ids::WorkspaceId;

/// One authenticated identity as the Host derived it.
#[derive(Debug, Clone)]
pub(crate) struct Principal {
    /// Wire projection.
    pub view: PrincipalView,
    /// Local OS user id when the transport proved one.
    pub local_uid: Option<u32>,
}

impl Principal {
    /// A local OS user proven by IPC peer credentials.
    pub fn local(uid: u32) -> Self {
        Self {
            view: PrincipalView {
                id: format!("local-user:{uid}"),
                kind: PrincipalKind::LocalUser,
                display: format!("local user {uid}"),
            },
            local_uid: Some(uid),
        }
    }
}

/// What a request intends to do. Controls are kept distinct because they
/// require stronger authorization and always produce an audit record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Read a projection.
    Read,
    /// Mutate session state.
    Mutate,
    /// Approve/reject a pending action.
    Approve,
    /// Stop the Host.
    Shutdown,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Mutate => "mutate",
            Self::Approve => "approve",
            Self::Shutdown => "shutdown",
        }
    }

    pub fn is_control(self) -> bool {
        matches!(self, Self::Approve | Self::Shutdown)
    }
}

/// Workspace-scoped authorization policy.
#[derive(Debug, Clone)]
pub(crate) struct Authz {
    workspace_id: WorkspaceId,
    local_uid: u32,
}

impl Authz {
    pub fn new(workspace_id: WorkspaceId) -> Self {
        Self {
            workspace_id,
            local_uid: current_uid(),
        }
    }

    pub fn local_uid(&self) -> u32 {
        self.local_uid
    }

    /// Authorize one request against the current policy.
    pub fn authorize(
        &self,
        principal: &Principal,
        target: &Target,
        _access: Access,
    ) -> Result<(), Rejection> {
        if target.workspace_id != self.workspace_id {
            return Err(denied("workspace is not served by this Host"));
        }
        match principal.view.kind {
            PrincipalKind::LocalUser => {
                if principal.local_uid == Some(self.local_uid) {
                    Ok(())
                } else {
                    Err(denied("local peer is not the Host user"))
                }
            }
            // Remote and device principals are not enabled in Stage 1. Fail
            // closed instead of inheriting the Host OS user's authority.
            PrincipalKind::Remote | PrincipalKind::Device => {
                Err(denied("remote principals are not enabled"))
            }
        }
    }
}

fn denied(message: &str) -> Rejection {
    Rejection {
        code: RejectionCode::Unauthorized,
        message: message.to_string(),
    }
}

#[cfg(unix)]
pub(crate) fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not touch memory.
    unsafe { libc::geteuid() as u32 }
}

#[cfg(not(unix))]
pub(crate) fn current_uid() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ids::SessionId;

    fn target(workspace: WorkspaceId) -> Target {
        Target {
            workspace_id: workspace,
            session_id: Some(SessionId::new()),
        }
    }

    #[test]
    fn local_user_is_confined_to_the_served_workspace() {
        let workspace = WorkspaceId::new();
        let authz = Authz::new(workspace);
        let principal = Principal::local(authz.local_uid());
        assert!(
            authz
                .authorize(&principal, &target(workspace), Access::Mutate)
                .is_ok()
        );
        let other = WorkspaceId::new();
        assert_eq!(
            authz
                .authorize(&principal, &target(other), Access::Read)
                .unwrap_err()
                .code,
            RejectionCode::Unauthorized
        );
    }

    #[test]
    fn remote_principals_never_inherit_local_authority() {
        let workspace = WorkspaceId::new();
        let authz = Authz::new(workspace);
        let remote = Principal {
            view: PrincipalView {
                id: "device:abc".into(),
                kind: PrincipalKind::Device,
                display: "phone".into(),
            },
            local_uid: None,
        };
        assert!(
            authz
                .authorize(&remote, &target(workspace), Access::Approve)
                .is_err()
        );
    }

    #[test]
    fn a_different_local_uid_is_denied() {
        let workspace = WorkspaceId::new();
        let authz = Authz::new(workspace);
        let stranger = Principal::local(authz.local_uid().wrapping_add(1));
        assert!(
            authz
                .authorize(&stranger, &target(workspace), Access::Read)
                .is_err()
        );
    }
}
