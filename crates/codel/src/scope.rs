//! Tenant, user, session, and workspace access boundaries.
//!
//! Every retrievable unit carries an [`Acl`]: who owns it, how widely it may
//! be shared, and which workspace it belongs to. A query carries an
//! [`AccessContext`]. [`AccessContext::allows`] is the single authority that
//! decides whether a unit may be returned. Retrieval, context assembly, and
//! the Elasticsearch-compatible surface all filter through it, so a unit
//! invisible to one caller is never leaked by another code path.
//!
//! Boundaries are enforced by identity comparison, not by path prefix or
//! string matching. Cross-tenant access is always denied, including for
//! [`Visibility::Tenant`] and [`Visibility::Workspace`] units.

use std::{fmt, str::FromStr};

use uuid::Uuid;

use crate::WorkspaceId;

macro_rules! scope_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Uuid);

        impl $name {
            /// Generate a fresh random identity.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Underlying UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

scope_id! {
    /// Owning tenant: the billing and data boundary.
    TenantId
}

scope_id! {
    /// Owning user within one tenant.
    UserId
}

scope_id! {
    /// Owning conversation/session within one user.
    SessionId
}

/// Identity of the principal a unit belongs to.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OwnerScope {
    /// Tenant boundary.
    pub tenant: TenantId,
    /// User inside the tenant.
    pub user: UserId,
    /// Optional session; absent for units that outlive one conversation.
    pub session: Option<SessionId>,
}

impl OwnerScope {
    /// Owner without a session.
    #[must_use]
    pub const fn durable(tenant: TenantId, user: UserId) -> Self {
        Self {
            tenant,
            user,
            session: None,
        }
    }

    /// Owner scoped to one session.
    #[must_use]
    pub const fn session(tenant: TenantId, user: UserId, session: SessionId) -> Self {
        Self {
            tenant,
            user,
            session: Some(session),
        }
    }
}

/// How widely a unit may be shared.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Visibility {
    /// Only the owning user, inside the owning session when one is set.
    Private,
    /// Any session of the owning user.
    User,
    /// Any member of the owning tenant with the same workspace.
    #[default]
    Workspace,
    /// Any member of the owning tenant.
    Tenant,
}

/// Access-control record attached to a retrievable unit.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Acl {
    /// Principal that owns the unit.
    pub owner: OwnerScope,
    /// Sharing scope.
    pub visibility: Visibility,
    /// Workspace the unit belongs to, when it is workspace-derived.
    pub workspace: Option<WorkspaceId>,
}

impl Acl {
    /// Workspace-visible record owned by `owner`.
    #[must_use]
    pub const fn workspace(owner: OwnerScope, workspace: WorkspaceId) -> Self {
        Self {
            owner,
            visibility: Visibility::Workspace,
            workspace: Some(workspace),
        }
    }

    /// Session-private record owned by `owner`.
    #[must_use]
    pub const fn private(owner: OwnerScope, workspace: Option<WorkspaceId>) -> Self {
        Self {
            owner,
            visibility: Visibility::Private,
            workspace,
        }
    }
}

/// Identity of one requester, bound to one workspace.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AccessContext {
    /// Requesting principal.
    pub owner: OwnerScope,
    /// Workspace the request is confined to.
    pub workspace: WorkspaceId,
}

impl AccessContext {
    /// Request context for a principal and workspace.
    #[must_use]
    pub const fn new(owner: OwnerScope, workspace: WorkspaceId) -> Self {
        Self { owner, workspace }
    }

    /// Whether this context may see a unit with the supplied ACL.
    ///
    /// Denial is the default: a workspace-derived unit is invisible when the
    /// requester is in another workspace, and nothing crosses a tenant.
    #[must_use]
    pub fn allows(&self, acl: &Acl) -> bool {
        if acl.owner.tenant != self.owner.tenant {
            return false;
        }
        if let Some(workspace) = acl.workspace
            && workspace != self.workspace
        {
            return false;
        }
        match acl.visibility {
            Visibility::Private => {
                acl.owner.user == self.owner.user
                    && match acl.owner.session {
                        Some(session) => Some(session) == self.owner.session,
                        None => true,
                    }
            }
            Visibility::User => acl.owner.user == self.owner.user,
            Visibility::Workspace => acl.workspace == Some(self.workspace),
            Visibility::Tenant => true,
        }
    }
}

/// A unit was requested under a context that may not see it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("access denied for the requested unit")]
pub struct AccessDenied;

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(tenant: TenantId, user: UserId, session: Option<SessionId>) -> AccessContext {
        AccessContext::new(
            OwnerScope {
                tenant,
                user,
                session,
            },
            WorkspaceId::new(),
        )
    }

    #[test]
    fn cross_tenant_is_always_denied() {
        let a = ctx(TenantId::new(), UserId::new(), None);
        let b = ctx(TenantId::new(), UserId::new(), None);
        let acl = Acl {
            owner: b.owner,
            visibility: Visibility::Tenant,
            workspace: None,
        };
        assert!(!a.allows(&acl));
    }

    #[test]
    fn private_units_need_the_owning_session() {
        let tenant = TenantId::new();
        let user = UserId::new();
        let session = SessionId::new();
        let owner = ctx(tenant, user, Some(session));
        let acl = Acl::private(OwnerScope::session(tenant, user, session), None);
        assert!(owner.allows(&acl));

        let other_session = ctx(tenant, user, Some(SessionId::new()));
        assert!(!other_session.allows(&acl));

        let durable = ctx(tenant, user, None);
        assert!(!durable.allows(&acl));
    }

    #[test]
    fn workspace_units_do_not_cross_workspaces() {
        let tenant = TenantId::new();
        let user = UserId::new();
        let workspace = WorkspaceId::new();
        let acl = Acl::workspace(OwnerScope::durable(tenant, user), workspace);

        let inside = AccessContext::new(OwnerScope::durable(tenant, user), workspace);
        assert!(inside.allows(&acl));

        let outside = AccessContext::new(OwnerScope::durable(tenant, user), WorkspaceId::new());
        assert!(!outside.allows(&acl));
    }

    #[test]
    fn tenant_units_are_shared_inside_one_tenant() {
        let tenant = TenantId::new();
        let writer = OwnerScope::durable(tenant, UserId::new());
        let reader = OwnerScope::durable(tenant, UserId::new());
        let workspace = WorkspaceId::new();
        let acl = Acl {
            owner: writer,
            visibility: Visibility::Tenant,
            workspace: None,
        };
        assert!(AccessContext::new(reader, workspace).allows(&acl));
    }

    #[test]
    fn user_units_are_not_shared_between_users() {
        let tenant = TenantId::new();
        let user = UserId::new();
        let other = UserId::new();
        let acl = Acl {
            owner: OwnerScope::durable(tenant, user),
            visibility: Visibility::User,
            workspace: None,
        };
        assert!(!ctx(tenant, other, None).allows(&acl));
    }
}
