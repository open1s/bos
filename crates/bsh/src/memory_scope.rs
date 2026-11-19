//! Memory scope resolution (issue #340).
//!
//! One resolver decides BOTH the storage identity (which file) and the access
//! scope (which records a session may read and write). Workspace memory is
//! isolated by the stable workspace registry id - a rename or move keeps the
//! memory - and the legacy machine-level `memory.jsonl` stays the User store,
//! read and upgraded in place, never destroyed or merged into a workspace file.
//!
//! Layout:
//! - workspace store: `<save_root>/memory/<workspace-id>.jsonl`, or the
//!   configured `memory_path` (relative paths resolve against the canonical
//!   workspace root; absolute paths are honored with workspace scope semantics);
//! - user store: `<save_root>/memory.jsonl` (the legacy default location).
//!
//! Recall and write both enforce the resolved scope BEFORE ranking or
//! injection; diagnostics expose the resolution, never memory contents.

use std::path::PathBuf;

use agent::memory::MemoryScope;

use crate::settings::Settings;
use crate::settings::resolve::SessionChoice;

/// The resolved storage identity and access scope for one session.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedMemory {
    /// The stable workspace identity (registry id, sanitized for a file name).
    pub(crate) workspace_id: String,
    /// The workspace-scoped store file.
    pub(crate) workspace_store: PathBuf,
    /// The legacy user-scoped store file (non-destructive compatibility).
    pub(crate) user_store: PathBuf,
    /// The scope the workspace store holds and migrates legacy lines into.
    pub(crate) storage_scope: MemoryScope,
    /// The session's viewer scope: reads see this chain, writes land here.
    pub(crate) viewer: MemoryScope,
    /// Whether an explicit `memory_path` configured the workspace store.
    pub(crate) custom: bool,
}

impl ResolvedMemory {
    /// A content-free diagnostic line naming the resolution (issue #340).
    pub(crate) fn diagnostic(&self) -> String {
        format!(
            "workspace={} viewer={:?} workspace_store={} user_store={} custom={}",
            self.workspace_id,
            self.viewer,
            self.workspace_store.display(),
            self.user_store.display(),
            self.custom
        )
    }
}

/// The storage half of a resolution, carried by the extension host.
///
/// The host binds one of these per build; each activation narrows the viewer
/// to its own session when the memory service is provided.
#[derive(Debug, Clone)]
pub(crate) struct MemoryBinding {
    /// The workspace-scoped store file.
    pub(crate) workspace_store: PathBuf,
    /// The scope the store holds and migrates legacy lines into.
    pub(crate) storage_scope: MemoryScope,
    /// The stable workspace identity.
    pub(crate) workspace_id: String,
}

/// Resolve the storage binding for a workspace path (no session yet).
///
/// The workspace registry id is matched by root path; a path no registry
/// entry names resolves as the default workspace, the same fallback the
/// session resolver uses.
pub(crate) fn binding_for(
    settings: &Settings,
    workspace: Option<&std::path::Path>,
) -> MemoryBinding {
    let id = workspace.and_then(|path| {
        settings
            .global
            .workspaces
            .iter()
            .find(|entry| std::path::Path::new(&entry.root) == path)
            .map(|entry| entry.id.clone())
    });
    let choice = SessionChoice {
        workspace: id,
        ..Default::default()
    };
    let resolved = MemoryScopeResolver::resolve(settings, &choice, "");
    MemoryBinding {
        workspace_store: resolved.workspace_store,
        storage_scope: resolved.storage_scope,
        workspace_id: resolved.workspace_id,
    }
}

/// The one place storage identity and access scope are resolved.
pub(crate) struct MemoryScopeResolver;

impl MemoryScopeResolver {
    /// Resolve the memory layout for a session in a workspace.
    pub(crate) fn resolve(
        settings: &Settings,
        choice: &SessionChoice,
        session: &str,
    ) -> ResolvedMemory {
        // The session's own workspace id wins; the first registered workspace
        // is the fallback for a session that names none. The id is the stable
        // identity: a workspace rename/move changes the root, not the memory.
        let entry = choice
            .workspace
            .as_ref()
            .and_then(|id| {
                settings
                    .global
                    .workspaces
                    .iter()
                    .find(|workspace| &workspace.id == id)
            })
            .or_else(|| settings.global.workspaces.first());
        let workspace_id = sanitize_id(
            entry
                .map(|workspace| workspace.id.as_str())
                .unwrap_or("default"),
        );
        let workspace_root = entry.map(|workspace| PathBuf::from(&workspace.root));

        let base = settings
            .save_root
            .clone()
            .unwrap_or_else(crate::settings::dir);
        let user_store = base.join("memory.jsonl");

        let raw = settings
            .workspace
            .agent
            .memory_path
            .as_deref()
            .unwrap_or("")
            .trim();
        let (workspace_store, custom) = if raw.is_empty() {
            (
                base.join("memory").join(format!("{workspace_id}.jsonl")),
                false,
            )
        } else {
            let expanded = PathBuf::from(shellexpand::tilde(raw).into_owned());
            let resolved = if expanded.is_absolute() {
                expanded
            } else {
                // A relative configured path resolves against the canonical
                // workspace root, never the process cwd (issue #340).
                workspace_root
                    .clone()
                    .unwrap_or_else(|| base.clone())
                    .join(expanded)
            };
            (resolved, true)
        };

        let storage_scope = MemoryScope::workspace(workspace_id.clone());
        let viewer = if session.trim().is_empty() {
            storage_scope.clone()
        } else {
            MemoryScope::session(workspace_id.clone(), session)
        };
        ResolvedMemory {
            workspace_id,
            workspace_store,
            user_store,
            storage_scope,
            viewer,
            custom,
        }
    }
}

/// The user identity the legacy user store is scoped to.
pub(crate) fn user_identity() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "local".to_string())
}

/// Keep a workspace id safe as one file-name component.
fn sanitize_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::global::WorkspaceRef;

    fn settings_with(root: &std::path::Path, workspaces: Vec<(&str, &str)>) -> Settings {
        let mut settings = Settings {
            save_root: Some(root.to_path_buf()),
            ..Default::default()
        };
        settings.global.workspaces = workspaces
            .into_iter()
            .map(|(id, ws_root)| WorkspaceRef {
                id: id.to_string(),
                root: ws_root.to_string(),
                ..Default::default()
            })
            .collect();
        settings
    }

    #[test]
    fn two_workspaces_resolve_to_isolated_stores() {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings_with(dir.path(), vec![("ws-a", "/a"), ("ws-b", "/b")]);
        let choice_a = SessionChoice {
            workspace: Some("ws-a".into()),
            ..Default::default()
        };
        let choice_b = SessionChoice {
            workspace: Some("ws-b".into()),
            ..Default::default()
        };
        let a = MemoryScopeResolver::resolve(&settings, &choice_a, "s1");
        let b = MemoryScopeResolver::resolve(&settings, &choice_b, "s1");
        assert_ne!(a.workspace_store, b.workspace_store);
        assert_eq!(
            a.user_store, b.user_store,
            "the user store is shared by identity"
        );
        assert_eq!(a.viewer, MemoryScope::session("ws-a", "s1"));
        assert!(
            !a.viewer.permits(&b.storage_scope),
            "no cross-workspace reads"
        );
    }

    #[test]
    fn a_workspace_rename_keeps_the_memory_identity() {
        let dir = tempfile::tempdir().unwrap();
        let before = settings_with(dir.path(), vec![("ws-a", "/old/path")]);
        let after = settings_with(dir.path(), vec![("ws-a", "/new/path")]);
        let choice = SessionChoice {
            workspace: Some("ws-a".into()),
            ..Default::default()
        };
        let a = MemoryScopeResolver::resolve(&before, &choice, "s");
        let b = MemoryScopeResolver::resolve(&after, &choice, "s");
        assert_eq!(
            a.workspace_store, b.workspace_store,
            "the id, not the path, is the identity"
        );
    }

    #[test]
    fn relative_custom_paths_resolve_against_the_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = settings_with(dir.path(), vec![("ws-a", "/projects/a")]);
        settings.workspace.agent.memory_path = Some("notes/memory.jsonl".into());
        let choice = SessionChoice {
            workspace: Some("ws-a".into()),
            ..Default::default()
        };
        let resolved = MemoryScopeResolver::resolve(&settings, &choice, "s");
        assert_eq!(
            resolved.workspace_store,
            PathBuf::from("/projects/a/notes/memory.jsonl")
        );
        assert!(resolved.custom);
    }

    #[test]
    fn absolute_custom_paths_are_honored() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = settings_with(dir.path(), vec![("ws-a", "/projects/a")]);
        settings.workspace.agent.memory_path = Some("/custom/memory.jsonl".into());
        let choice = SessionChoice::default();
        let resolved = MemoryScopeResolver::resolve(&settings, &choice, "s");
        assert_eq!(
            resolved.workspace_store,
            PathBuf::from("/custom/memory.jsonl")
        );
    }

    #[test]
    fn the_legacy_user_store_keeps_its_location() {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings_with(dir.path(), vec![]);
        let resolved = MemoryScopeResolver::resolve(&settings, &SessionChoice::default(), "");
        assert_eq!(resolved.user_store, dir.path().join("memory.jsonl"));
        assert_eq!(resolved.workspace_id, "default");
        assert_eq!(resolved.viewer, MemoryScope::workspace("default"));
    }

    #[test]
    fn ids_are_sanitized_for_file_names() {
        assert_eq!(sanitize_id("../../etc"), "etc");
        assert_eq!(sanitize_id("ws 1"), "ws_1");
        assert_eq!(sanitize_id("///"), "default");
        assert_eq!(sanitize_id("good-id_1"), "good-id_1");
    }

    #[test]
    fn diagnostics_name_the_resolution_without_content() {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings_with(dir.path(), vec![("ws-a", "/a")]);
        let choice = SessionChoice {
            workspace: Some("ws-a".into()),
            ..Default::default()
        };
        let resolved = MemoryScopeResolver::resolve(&settings, &choice, "s1");
        let line = resolved.diagnostic();
        assert!(line.contains("workspace=ws-a"));
        assert!(line.contains("memory.jsonl"));
    }
}
