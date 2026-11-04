//! Windows AppContainer launch path for extension children.
//!
//! The Job Object in [`crate::windows_job`] caps memory and guarantees the
//! child dies with the runtime, but it does not restrict what the child can
//! reach. This module adds the other half: the child is launched into an
//! AppContainer through heel, with read access to its package and write access
//! only to the workspace and its data namespace, and no network. The AppContainer
//! profile, filesystem ACLs, job object and attributed `CreateProcessW` are
//! heel's, so there is one sandbox implementation in the project.

use std::path::Path;

use heel::{Access, DenyAll, Sandbox, SandboxConfig, StdioConfig};

/// Environment variables a Windows child needs to find its runtime and temp
/// directory. Everything else is cleared by the sandbox.
const PASSTHROUGH: &[&str] = &[
    "PATH",
    "SystemRoot",
    "WINDIR",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "LOCALAPPDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "COMSPEC",
];

/// A live AppContainer sandbox. Kept for the lifetime of the runtime because
/// its process tracker kills every child it launched when it drops.
pub(crate) struct WindowsSandbox {
    sandbox: Sandbox<DenyAll>,
}

impl WindowsSandbox {
    /// Build the sandbox for one extension.
    pub(crate) async fn new(
        root: &Path,
        workspace: Option<&Path>,
        data: Option<&Path>,
        memory_bytes: u64,
    ) -> Result<Self, String> {
        let limits = heel::ResourceLimits::builder()
            .max_memory_bytes(memory_bytes)
            .build();
        let mut builder = SandboxConfig::builder()
            .filesystem_strict(true)
            .limits(limits)
            .working_dir(root)
            .readable(root)
            .executable(root)
            .env_passthroughs(PASSTHROUGH.iter().copied());
        if let Some(workspace) = workspace {
            builder = builder.writable(workspace);
        }
        if let Some(data) = data {
            builder = builder.writable(data);
        }
        let sandbox = Sandbox::with_config(builder.build())
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self { sandbox })
    }

    /// Build the sandbox from a synchronous caller (the MCP runtime's
    /// constructor). heel's executor is smol-based, so this does not need a
    /// tokio reactor.
    pub(crate) fn new_blocking(
        root: &Path,
        workspace: Option<&Path>,
        data: Option<&Path>,
        memory_bytes: u64,
    ) -> Result<Self, String> {
        futures_lite::future::block_on(Self::new(root, workspace, data, memory_bytes))
    }

    /// Spawn one child with piped stdio and the caller's environment.
    pub(crate) async fn spawn(
        &self,
        program: &str,
        args: &[String],
        cwd: &Path,
        envs: &[(String, String)],
    ) -> Result<heel::Child, String> {
        let mut command = self
            .sandbox
            .command(program)
            .current_dir(cwd)
            .stdin(StdioConfig::Piped)
            .stdout(StdioConfig::Piped)
            .stderr(StdioConfig::Piped);
        for arg in args {
            command = command.arg(arg);
        }
        for (key, value) in envs {
            command = command.env(key, value);
        }
        command.spawn().await.map_err(|error| error.to_string())
    }

    /// Run a command to completion (used by the Windows isolation tests).
    #[cfg(test)]
    pub(crate) async fn output(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
    ) -> Result<std::process::Output, String> {
        let mut command = self
            .sandbox
            .command(program)
            .current_dir(cwd)
            .stdin(StdioConfig::Null)
            .stdout(StdioConfig::Piped)
            .stderr(StdioConfig::Piped);
        for arg in args {
            command = command.arg(arg);
        }
        command.output().await.map_err(|error| error.to_string())
    }
}

/// The write grant a package gets is not the whole filesystem: prove it with a
/// command that tries both sides of the boundary.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_appcontainer_child_can_write_its_workspace_but_not_outside_it() {
        let scratch = std::env::temp_dir().join(format!("bsh-appcontainer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let root = scratch.join("package");
        let workspace = scratch.join("workspace");
        let outside = scratch.join("outside");
        std::fs::create_dir_all(&root).expect("package");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(root.join("payload.txt"), b"inside").expect("payload");

        let sandbox = WindowsSandbox::new_blocking(&root, Some(&workspace), None, 64 * 1024 * 1024)
            .expect("sandbox");

        let read = futures_lite::future::block_on(sandbox.output(
            "cmd",
            &["/C", "type payload.txt"],
            &root,
        ))
        .expect("run read");
        assert!(
            String::from_utf8_lossy(&read.stdout).contains("inside"),
            "the sandbox must read its package: {}",
            String::from_utf8_lossy(&read.stderr)
        );

        let write = futures_lite::future::block_on(sandbox.output(
            "cmd",
            &["/C", "echo ok > granted.txt"],
            &workspace,
        ))
        .expect("run write");
        assert!(
            workspace.join("granted.txt").is_file(),
            "workspace write: {write:?}"
        );

        let denied = futures_lite::future::block_on(sandbox.output(
            "cmd",
            &[
                "/C",
                &format!("echo no > \"{}\"", outside.join("leak.txt").display()),
            ],
            &root,
        ))
        .expect("run denied write");
        assert!(
            !outside.join("leak.txt").exists(),
            "an AppContainer child must not write outside its grants: {denied:?}"
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }
}
