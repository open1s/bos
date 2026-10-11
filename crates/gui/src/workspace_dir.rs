//! The workspace's own `.bos` folder: configuration that lives next to the code.
//!
//! `~/.bos/gui/settings.toml` is the **registry** — which workspaces exist, which
//! one is shown, and the device-level defaults. This module gives every workspace
//! a second home, in its own root, the way a project's `.vscode` folder holds the
//! settings that only make sense for that project:
//!
//! - adding a workspace **creates** `<root>/.bos/` and writes `workspace.toml`
//!   there, so the folder the user picked owns its configuration instead of the
//!   app's user-level store being its only home;
//! - editing the workspace rewrites the same file, so the two copies agree;
//! - loading settings reads it back, so a folder that arrives with its `.bos`
//!   (cloned, copied, restored from a backup) arrives configured.
//!
//! Three rules keep that safe to do inside someone's project directory:
//!
//! - **Identity never travels.** `id`, `name`, `root` and the timestamps stay
//!   in the registry: `workspace.toml` says how a folder runs, not which records
//!   point at it, so the same file in a clone is a configuration rather than a
//!   second workspace.
//! - **Secrets never travel.** The API key, the fallback providers (which carry
//!   keys of their own) and the MCP **entries** (whose `args` and `url` can
//!   hold a token) are not written. What travels about MCP is the list of
//!   **names** a workspace has switched on, so the file can turn an existing
//!   server on or off and can never introduce a command this installation did not
//!   already have. The file is therefore safe to commit.
//! - **A file is input, never an instruction.** One that will not parse, a field
//!   it does not carry, and an MCP name this installation does not know each
//!   leave the workspace's own value alone rather than failing the load; a field
//!   the file *does* carry wins over the registry copy, which is what makes the
//!   folder the source of truth for the workspace rooted in it.
//!
//! Writing is atomic (a sibling temporary file, then a rename), so a crash
//! mid-write cannot leave a half-parsed configuration where the next load reads
//! it. Removing a workspace from the app leaves this folder alone, the way
//! removing a project from an editor leaves `.vscode`: the folder is the user's,
//! and its configuration may belong to the next workspace too. The **empty** root
//! (the process working directory) is the one root with no folder of its own:
//! `write_for` and `load_for` skip it in both directions rather than resolving it,
//! because the directory a bundled app is launched in is not one the user picked —
//! and a file written to the launch directory would configure that workspace, or
//! fail to, depending on how the app was started, which is not a home.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::settings::{McpServerEntry, Workspace};

/// Name of the per-workspace configuration folder, created in the root.
pub(crate) const BOS_DIR: &str = ".bos";

/// Configuration file inside it.
pub(crate) const CONFIG_FILE: &str = "workspace.toml";

/// The folder a workspace's configuration lives in: `<root>/.bos`.
///
/// An empty root means the process working directory, and this still names a path
/// for it — the *definition* is the one `Settings::effective` applies when it
/// resolves the root for the tools — but `write_for` and `load_for` decline to
/// use it: the app does not own the folder it happened to be launched in.
pub(crate) fn dir_for(root: &str) -> PathBuf {
    root_path(root).join(BOS_DIR)
}

/// The configuration file's path: `<root>/.bos/workspace.toml`.
pub(crate) fn config_path(root: &str) -> PathBuf {
    dir_for(root).join(CONFIG_FILE)
}

/// The root as a path: the workspace's own when it has one, else the process
/// working directory.
fn root_path(root: &str) -> PathBuf {
    let trimmed = root.trim();
    if trimmed.is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        PathBuf::from(shellexpand::tilde(trimmed).into_owned())
    }
}

/// The workspace's configuration as the folder file carries it.
///
/// Every field is an `Option` so "the file does not mention this" and "the file
/// sets this to empty/false" stay distinguishable: the first leaves the
/// registry's value in place, the second replaces it. The fields the file
/// deliberately has no key for are the ones that cannot travel — identity
/// (`id`/`name`/`root`, timestamps) and secrets (`api_key`, the providers, the
/// MCP connection entries).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct WorkspaceFile {
    /// Model id or `[llm.<name>]` profile; absent means inherit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    /// API base URL; absent means inherit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) base_url: Option<String>,
    /// System prompt; absent means inherit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) system_prompt: Option<String>,
    /// Sampling temperature; absent means inherit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f32>,
    /// Reasoning effort; absent means inherit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_effort: Option<String>,
    /// Whether the bash tool is registered here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bash_enabled: Option<bool>,
    /// Whether the filesystem tools are registered here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file_tools_enabled: Option<bool>,
    /// Whether `bash`/`write_file` need one-click approval here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) require_approval: Option<bool>,
    /// Send-side context budget in tokens; 0 disables compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) context_budget: Option<usize>,
    /// Whether this workspace folds `AGENTS.md`-style files into its prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project_instructions: Option<bool>,
    /// Whether a persistent memory store is attached here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) memory_enabled: Option<bool>,
    /// Memory JSON-lines file; absent means the shared default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) memory_path: Option<String>,
    /// Directory scanned for `SKILL.md` folders; absent means the shared one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skills_dir: Option<String>,
    /// Built-in tools switched off here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disabled_tools: Option<Vec<String>>,
    /// Skills switched off here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disabled_skills: Option<Vec<String>>,
    /// Plugins switched off here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disabled_plugins: Option<Vec<String>>,
    /// Names of the MCP servers this workspace has switched on — names only, so
    /// a file can never name a command, an argument or a URL this installation
    /// does not already have. Absent leaves every seeded switch as it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mcp_enabled: Option<Vec<String>>,
}

impl WorkspaceFile {
    /// Read a workspace's settings into the fields the file carries.
    ///
    /// Everything the file does not mention — and every MCP entry, whose switch
    /// this only flips by name — keeps the value it already had.
    pub(crate) fn from_workspace(w: &Workspace) -> Self {
        Self {
            model: non_empty(&w.model),
            base_url: non_empty(&w.base_url),
            system_prompt: non_empty(&w.system_prompt),
            temperature: w.temperature,
            reasoning_effort: w.reasoning_effort.clone(),
            bash_enabled: Some(w.bash_enabled),
            file_tools_enabled: Some(w.file_tools_enabled),
            require_approval: Some(w.require_approval),
            context_budget: Some(w.context_budget),
            project_instructions: Some(w.project_instructions),
            memory_enabled: Some(w.memory_enabled),
            memory_path: non_empty(&w.memory_path),
            skills_dir: non_empty(&w.skills_dir),
            disabled_tools: Some(w.disabled_tools.clone()),
            disabled_skills: Some(w.disabled_skills.clone()),
            disabled_plugins: Some(w.disabled_plugins.clone()),
            mcp_enabled: Some(enabled_mcp_names(&w.mcp_servers)),
        }
    }

    /// Fold the file's settings over a workspace record.
    ///
    /// The identity fields and the secrets are untouched by construction (this
    /// struct has no key for them). An MCP name this installation does not have
    /// matches no entry, so the rule stays "exactly the names in the file are
    /// on" without a file ever being able to add a server.
    pub(crate) fn apply_to(&self, w: &mut Workspace) {
        if let Some(v) = &self.model {
            w.model = v.clone();
        }
        if let Some(v) = &self.base_url {
            w.base_url = v.clone();
        }
        if let Some(v) = &self.system_prompt {
            w.system_prompt = v.clone();
        }
        if let Some(v) = self.temperature {
            w.temperature = Some(v);
        }
        if let Some(v) = &self.reasoning_effort {
            w.reasoning_effort = non_empty(v);
        }
        if let Some(v) = self.bash_enabled {
            w.bash_enabled = v;
        }
        if let Some(v) = self.file_tools_enabled {
            w.file_tools_enabled = v;
        }
        if let Some(v) = self.require_approval {
            w.require_approval = v;
        }
        if let Some(v) = self.context_budget {
            w.context_budget = v;
        }
        if let Some(v) = self.project_instructions {
            w.project_instructions = v;
        }
        if let Some(v) = self.memory_enabled {
            w.memory_enabled = v;
        }
        if let Some(v) = &self.memory_path {
            w.memory_path = v.clone();
        }
        if let Some(v) = &self.skills_dir {
            w.skills_dir = v.clone();
        }
        if let Some(v) = &self.disabled_tools {
            w.disabled_tools = v.clone();
        }
        if let Some(v) = &self.disabled_skills {
            w.disabled_skills = v.clone();
        }
        if let Some(v) = &self.disabled_plugins {
            w.disabled_plugins = v.clone();
        }
        if let Some(names) = &self.mcp_enabled {
            for server in &mut w.mcp_servers {
                server.enabled = names.iter().any(|n| n == &server.name);
            }
        }
    }
}

/// Write one workspace's configuration into its own `.bos` folder, creating the
/// folder when it is not there yet, and answer with the file's path.
///
/// Three answers, because three situations are genuinely different. An **empty
/// root** is the process working directory — the folder the app happens to run
/// in, not one the user picked, and not a stable identity for the workspace either
/// — so nothing is written there and the answer is `None`; a root that is *not a directory* is the same answer, because a preset
/// may name a folder that has since moved and `Settings::apply_preset` has never
/// validated a root. A root that *is* there either takes the file
/// (`Some(path)`) or is a refusal: creation treats that as fatal, because the
/// folder the user picked could not take its configuration and a workspace that
/// silently has no `.bos` is the state this module exists to prevent, while the
/// edit path restores the record it was about to change, so a refused write
/// cannot leave the app configured differently from the folder's file.
pub(crate) fn write_for(w: &Workspace) -> Result<Option<PathBuf>, String> {
    // The empty root is checked before anything else, and deliberately. Two
    // reasons, either of them sufficient. The working directory of a bundled app
    // can be `/`, where creating a folder is not merely impolite but a permission
    // refusal that would take the whole creation down with it. And the deeper one:
    // a directory the app was merely *started* in is not a stable identity for a
    // workspace, so a configuration written there would apply — or not — depending
    // on where the next launch happened, which is a worse answer than none.
    let root = w.root.trim();
    if root.is_empty() || !root_path(root).is_dir() {
        return Ok(None);
    }
    let path = config_path(root);
    let dir = path
        .parent()
        .expect("a configuration path always has a parent")
        .to_path_buf();
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut text = String::from(
        "# BOS workspace configuration, next to the code (like .vscode).\n\
         # Secrets are not written here: the API key and the MCP connection\n\
         # details stay in ~/.bos/gui/settings.toml.\n",
    );
    text.push_str(
        &toml::to_string_pretty(&WorkspaceFile::from_workspace(w))
            .map_err(|e| format!("{}: {e}", path.display()))?,
    );
    let tmp = dir.join(format!("{CONFIG_FILE}.tmp"));
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(Some(path))
}

/// Read a workspace's own configuration and fold it over the record, answering
/// whether a file was applied.
///
/// A missing file, an unreadable one and one that will not parse are all `false`
/// rather than errors: loading the app must not fail because a hand-edited file
/// next to someone's code has a typo, and the registry's copy is the workspace's
/// value until a file says otherwise.
pub(crate) fn load_for(w: &mut Workspace) -> bool {
    // Symmetrical with `write_for`: a folder nobody picked is neither prepared nor
    // read, so a stray `.bos` in whatever directory the app was launched from
    // cannot configure the workspace that stands for "here".
    let root = w.root.trim();
    if root.is_empty() {
        return false;
    }
    let Ok(text) = fs::read_to_string(config_path(root)) else {
        return false;
    };
    let Ok(file) = toml::from_str::<WorkspaceFile>(&text) else {
        return false;
    };
    file.apply_to(w);
    true
}

/// The names of the MCP servers switched on, in list order.
fn enabled_mcp_names(servers: &[McpServerEntry]) -> Vec<String> {
    servers
        .iter()
        .filter(|s| s.enabled)
        .map(|s| s.name.clone())
        .collect()
}

/// A string as the file carries it: absent when blank, so "inherit" keeps one
/// representation in the registry and none in the file.
fn non_empty(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ProviderEntry, Settings};
    use std::path::Path;

    /// A root this test owns; anything an earlier run left there is removed
    /// first, because an assertion about a folder is an assertion about the
    /// whole folder.
    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bos-gui-wsdir-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// A workspace that exercises both halves of the rule: settings that travel
    /// and secrets and identity that must not.
    fn workspace_at(root: &Path) -> Workspace {
        let mut w = Workspace {
            id: "w-1".to_string(),
            name: "demo".to_string(),
            root: root.to_string_lossy().into_owned(),
            ..Default::default()
        };
        w.model = "glm-4.7".to_string();
        w.base_url = "https://api.example/v1".to_string();
        w.api_key = "sk-primary-secret".to_string();
        w.providers = vec![ProviderEntry {
            name: "backup".to_string(),
            base_url: "https://backup.example/v1".to_string(),
            api_key: "sk-fallback-secret".to_string(),
            model: "vendor/backup".to_string(),
            enabled: true,
        }];
        w.mcp_servers = vec![
            McpServerEntry {
                name: "github".to_string(),
                transport: "stdio".to_string(),
                command: "/opt/gh-mcp".to_string(),
                args: vec!["--token".to_string(), "tok-secret".to_string()],
                ..Default::default()
            },
            McpServerEntry {
                name: "remote".to_string(),
                transport: "http".to_string(),
                url: "https://mcp.example/sse?key=tok-secret".to_string(),
                enabled: false,
                ..Default::default()
            },
        ];
        w.disabled_tools = vec!["bash".to_string()];
        w.skills_dir = "/opt/skills".to_string();
        w
    }

    #[test]
    fn a_created_folder_carries_the_settings_a_second_read_gets_back() {
        let root = temp_root("roundtrip");
        let w = workspace_at(&root);
        let path = write_for(&w)
            .expect("written")
            .expect("a root with a folder gets the file");
        assert_eq!(path, root.join(BOS_DIR).join(CONFIG_FILE));
        assert!(path.is_file(), "the folder gets a file, not just a folder");
        // A record that carries the switches wrong picks the folder's answer up:
        // the file wins for every field it has a key for.
        let mut other = workspace_at(&root);
        other.model = String::new();
        other.base_url = String::new();
        other.disabled_tools = Vec::new();
        other.mcp_servers[0].enabled = false;
        other.mcp_servers[1].enabled = true;
        assert!(load_for(&mut other), "the file is applied");
        assert_eq!(other.model, "glm-4.7");
        assert_eq!(other.base_url, "https://api.example/v1");
        assert_eq!(other.disabled_tools, vec!["bash".to_string()]);
        assert_eq!(other.skills_dir, "/opt/skills");
        assert!(
            other.mcp_servers[0].enabled,
            "the file's list turns github on"
        );
        assert!(
            !other.mcp_servers[1].enabled,
            "and turns off every server it does not name"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_file_carries_no_secret_and_no_identity() {
        let root = temp_root("secrets");
        let w = workspace_at(&root);
        let path = write_for(&w)
            .expect("written")
            .expect("a root with a folder gets the file");
        let text = fs::read_to_string(&path).expect("readable");
        for secret in [
            "sk-primary-secret",
            "sk-fallback-secret",
            "tok-secret",
            "/opt/gh-mcp",
            "https://mcp.example/sse",
        ] {
            assert!(
                !text.contains(secret),
                "{secret:?} must not be in a file that travels with the code"
            );
        }
        let parsed: toml::Value = toml::from_str(&text).expect("valid toml");
        let table = parsed.as_table().expect("a table");
        for key in [
            "id",
            "name",
            "root",
            "api_key",
            "providers",
            "mcp_servers",
            "created_at",
            "updated_at",
        ] {
            assert!(
                !table.contains_key(key),
                "{key} must not be a key in the folder's file"
            );
        }
        assert!(table.contains_key("mcp_enabled"), "the switches do travel");
        assert_eq!(
            table["mcp_enabled"].as_array().map(Vec::len),
            Some(1),
            "as names — one here, the enabled one"
        );
        // A read cannot touch what the file has no key for.
        let mut target = workspace_at(&root);
        target.id = "w-2".to_string();
        target.name = "renamed".to_string();
        target.api_key = "sk-live".to_string();
        target.providers[0].api_key = "sk-live-fallback".to_string();
        assert!(load_for(&mut target));
        assert_eq!(target.id, "w-2");
        assert_eq!(target.name, "renamed");
        assert_eq!(target.api_key, "sk-live");
        assert_eq!(target.providers[0].api_key, "sk-live-fallback");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_absent_or_unreadable_file_leaves_the_workspace_alone() {
        let root = temp_root("absent");
        let mut w = workspace_at(&root);
        assert!(!load_for(&mut w), "there is no file yet");
        assert_eq!(w.model, "glm-4.7");

        fs::create_dir_all(dir_for(&w.root)).expect("the folder");
        fs::write(config_path(&w.root), "this is not = = toml").expect("garbage");
        let before = w.clone();
        assert!(
            !load_for(&mut w),
            "a file that will not parse is not an instruction"
        );
        assert_eq!(w, before, "and it changed nothing on its way out");

        // One workspace's broken file does not hide another's good one: the
        // loader counts what it actually applied.
        let mut settings = Settings::from_config_value(&serde_json::Value::Null);
        settings.workspaces = vec![workspace_at(&root)];
        let good = temp_root("absent-other");
        settings.workspaces.push(workspace_at(&good));
        write_for(&settings.workspaces[1]).expect("written");
        assert_eq!(settings.adopt_workspace_dirs(), 1);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&good);
    }

    #[test]
    fn an_mcp_name_this_installation_does_not_have_switches_nothing_on() {
        let root = temp_root("ghost");
        let w = workspace_at(&root);
        fs::create_dir_all(dir_for(&w.root)).expect("the folder");
        fs::write(config_path(&w.root), "mcp_enabled = [\"ghost\"]\n").expect("hand-written");
        let mut target = workspace_at(&root);
        assert!(load_for(&mut target));
        assert!(
            target.mcp_servers.iter().all(|s| !s.enabled),
            "the rule is exactly these are on, and none of these exists"
        );
        let names: Vec<&str> = target.mcp_servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["github", "remote"], "and none was invented");
        // A field the file does not mention keeps the registry's value rather
        // than being reset to a default.
        assert_eq!(target.model, "glm-4.7");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_that_cannot_take_a_bos_directory_is_refused() {
        let root = temp_root("blocked");
        // A plain file where the folder has to go: deterministic on every
        // platform and for every user, unlike a permission bit that a
        // root-owned test runner ignores.
        fs::write(root.join(BOS_DIR), b"not a directory").expect("the blocking file");
        let w = workspace_at(&root);
        let err = write_for(&w).expect_err("refused");
        assert!(err.contains(BOS_DIR), "the reason names the folder: {err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_root_is_not_a_folder_the_user_picked() {
        // The probed path is the folder the app runs in — not one this test owns —
        // and both answers have to be distinguishable there: a refused write must
        // leave the file that is already in it alone, and a refused read must not
        // apply it. So the path is lent a file, and the guard puts it back however
        // this ends: a panic here must not leave a configuration behind for the
        // next run to read, which is the same rule as "a test that asserts a count
        // owns its file", one step further out.
        struct Lend(PathBuf, Option<String>, bool);
        impl Drop for Lend {
            fn drop(&mut self) {
                match &self.1 {
                    Some(text) => {
                        let _ = fs::write(&self.0, text);
                    }
                    None => {
                        let _ = fs::remove_file(&self.0);
                        if !self.2 {
                            if let Some(folder) = self.0.parent() {
                                let _ = fs::remove_dir(folder);
                            }
                        }
                    }
                }
            }
        }
        let root = temp_root("empty-root");
        let cwd = std::env::current_dir().expect("cwd");
        let folder = cwd.join(BOS_DIR);
        let probe = folder.join(CONFIG_FILE);
        let previous = fs::read_to_string(&probe).ok();
        let folder_existed = folder.exists();
        fs::create_dir_all(&folder).expect("the launch folder's .bos");
        fs::write(&probe, "model = \"stray/model\"\n").expect("a stray configuration");
        let _lend = Lend(probe.clone(), previous, folder_existed);
        let stray = fs::read_to_string(&probe).expect("written");

        let mut empty = workspace_at(&root);
        empty.root = String::new();
        assert_eq!(
            write_for(&empty),
            Ok(None),
            "an empty root has no folder of its own to prepare"
        );
        assert_eq!(
            fs::read_to_string(&probe).expect("still there"),
            stray,
            "and it did not rewrite the file the launch folder already holds"
        );
        let mut target = empty.clone();
        target.model = "registry/model".to_string();
        assert!(
            !load_for(&mut target),
            "an empty root reads nothing out of the launch folder"
        );
        assert_eq!(
            target.model, "registry/model",
            "not even the model that stray file names"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_second_write_replaces_the_first_and_leaves_no_temporary() {
        let root = temp_root("rewrite");
        let mut w = workspace_at(&root);
        write_for(&w).expect("first");
        w.model = "glm-4.8".to_string();
        write_for(&w).expect("second");
        let mut read = workspace_at(&root);
        assert!(load_for(&mut read));
        assert_eq!(read.model, "glm-4.8");
        let mut left: Vec<String> = fs::read_dir(dir_for(&w.root))
            .expect("listed")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec![CONFIG_FILE.to_string()]);
        let _ = fs::remove_dir_all(&root);
    }
}
