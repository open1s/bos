//! codel MCP server: stdio by default, optional JSON-RPC over HTTP.

mod ast;
mod context;
mod diagnostics;
mod http;
mod protocol;
mod server;
mod workspace;

use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("--http") => {
            let address = args
                .get(2)
                .cloned()
                .unwrap_or_else(|| "127.0.0.1:8765".to_owned());
            http::run(&address)
        }
        Some("--refresh") => refresh_once(),
        Some("--watch") => watch_loop(),
        Some("--hook") => install_hooks(args.get(2).map(String::as_str)),
        _ => run_stdio(),
    };
    if let Err(error) = result {
        eprintln!("codel-mcp: {error}");
        std::process::exit(1);
    }
}

fn host() -> Result<workspace::HostContext, String> {
    workspace::host_from_env()
        .ok_or_else(|| "CODEL_WORKSPACE_ID is required for this mode".to_owned())
}

fn refresh_once() -> Result<(), String> {
    let host = host()?;
    let root = host
        .root
        .clone()
        .ok_or_else(|| "CODEL_ROOT is required for refresh".to_owned())?;
    let graph = workspace::load_workspace(&host, &root, None)?;
    eprintln!(
        "codel-mcp: refreshed {} nodes, {} edges (source: {})",
        graph.graph.nodes().count(),
        graph.graph.edges().len(),
        if graph.loaded_from_store {
            "store"
        } else {
            "index"
        }
    );
    Ok(())
}

fn watch_loop() -> Result<(), String> {
    let host = host()?;
    let root = host
        .root
        .clone()
        .ok_or_else(|| "CODEL_ROOT is required for watch".to_owned())?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |result: notify::Result<notify::Event>| {
            if result.is_ok() {
                let _ = sender.send(());
            }
        },
        notify::Config::default(),
    )
    .map_err(|error| error.to_string())?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|error| error.to_string())?;
    loop {
        if receiver.recv().is_err() {
            break;
        }
        // Coalesce editor save bursts into one refresh.
        std::thread::sleep(Duration::from_millis(250));
        while receiver.try_recv().is_ok() {}
        if let Err(error) = refresh_once() {
            eprintln!("codel-mcp: watch refresh failed: {error}");
        }
    }
    Ok(())
}

fn install_hooks(root: Option<&str>) -> Result<(), String> {
    let root = root
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().map_err(|error| error.to_string())?);
    let git = root.join(".git");
    if !git.is_dir() {
        return Err(format!("not a git repository: {}", root.display()));
    }
    let state = root.join(".codel");
    std::fs::create_dir_all(&state).map_err(|error| error.to_string())?;
    let id_path = state.join("workspace-id");
    if !id_path.is_file() {
        std::fs::write(&id_path, codel::WorkspaceId::new().to_string())
            .map_err(|error| error.to_string())?;
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let store =
        std::env::var("CODEL_STORE_ROOT").unwrap_or_else(|_| "$HOME/.codel/store".to_owned());
    let script = format!(
        concat!(
            "#!/bin/sh\n",
            "# codel-mcp refresh\n",
            "ROOT=\"$(git rev-parse --show-toplevel 2>/dev/null)\" || exit 0\n",
            "ID=\"$(cat \"$ROOT/.codel/workspace-id\" 2>/dev/null)\" || exit 0\n",
            "CODEL_WORKSPACE_ID=\"$ID\" CODEL_ROOT=\"$ROOT\" ",
            "CODEL_STORE_ROOT=\"{}\" \"{}\" --refresh >/dev/null 2>&1 || true\n"
        ),
        store,
        executable.display()
    );
    for hook in ["post-commit", "post-checkout", "post-merge"] {
        install_hook(&git.join("hooks").join(hook), &script)?;
    }
    eprintln!(
        "codel-mcp: installed post-commit, post-checkout, post-merge hooks in {}",
        git.display()
    );
    Ok(())
}

fn install_hook(path: &Path, script: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let marker = "# codel-mcp refresh";
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    if !existing.contains(marker) {
        let mut content = existing;
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(script);
        std::fs::write(path, content).map_err(|error| error.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .map_err(|error| error.to_string())?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn run_stdio() -> Result<(), String> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut server = server::McpServer::new(stdout.lock());
    server.enable_watch();

    if let Some(host) = workspace::host_from_env() {
        server.set_host(host);
    }

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(message) => {
                if let Err(error) = server.handle(message) {
                    eprintln!("codel-mcp: {error}");
                    break;
                }
            }
            Err(error) => {
                let _ = server.send(protocol::error(
                    Value::Null,
                    -32700,
                    format!("parse error: {error}"),
                ));
            }
        }
    }
    Ok(())
}
