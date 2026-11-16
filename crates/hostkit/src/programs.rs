//! Where an app finds user-installed programs without a shell `PATH`.
//!
//! A GUI application does not inherit the shell environment: a Finder-launched
//! macOS app gets launchd's minimal path, and a process started from Explorer
//! on Windows gets only the machine variables. The version managers (mise,
//! nvm, asdf, fnm, Volta, pnpm) keep their programs in per-user directories a
//! shellless launch never sees, so every runtime that resolves an external
//! program would otherwise repeat - and drift on - the same platform list.
//!
//! This module is that one list. A version-managed interpreter is resolved to
//! the **versioned binary** the manager installed, not first to its shim: a
//! shim reads the manager's own configuration and can refuse to run when it is
//! spawned outside a shell, while the versioned binary it would have selected
//! is directly spawnable. Shims stay in the list as a later fallback, for an
//! install whose data directory this module does not know.
//!
//! Before scanning directories, [`managed_interpreter`] asks an installed
//! manager which interpreter *it* selects (`mise which node`, `asdf which node`,
//! `volta which node`), so the manager's own configuration - global, project or
//! environment - stays in charge instead of being guessed from the newest
//! installed version. The query is bounded; a manager that is absent, fails or
//! does not answer in time falls through to the scan.
//!
//! Every candidate is a directory; the caller probes each for a regular file,
//! so a directory that does not exist costs one failed check, never an error.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::home::home_dir;

/// How long a version manager may take to name its interpreter.
const MANAGER_TIMEOUT: Duration = Duration::from_millis(1500);

/// The interpreter an installed version manager selects for `program`.
///
/// `mise`, `asdf` and `volta` answer `<tool> which <program>`; asking them is
/// the only way to honour a version the manager activates through its own
/// configuration or environment. The query is bounded by
/// [`MANAGER_TIMEOUT`] and its output must name an existing file; otherwise
/// this returns `None` and the caller keeps searching directories.
pub fn managed_interpreter(program: &str) -> Option<PathBuf> {
    managed_interpreter_with(program, &environment)
}

/// [`managed_interpreter`] with the environment lookup injected for tests.
fn managed_interpreter_with(
    program: &str,
    env: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    for manager in ["mise", "asdf", "volta"] {
        let Some(executable) = find_manager(manager, env) else {
            continue;
        };
        if let Some(path) = ask_manager(&executable, program) {
            return Some(path);
        }
    }
    None
}

/// The manager executable itself, found the same way an interpreter is.
fn find_manager(name: &str, env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let mut directories = Vec::new();
    if let Some(path) = env("PATH") {
        directories.extend(std::env::split_paths(&path));
    }
    if let Some(home) = home_dir() {
        directories.extend(user_directories(&home, env));
    }
    find_in(&directories, &executable_names(name))
}

/// Ask one manager, bounded, and accept only an existing file.
fn ask_manager(manager: &Path, program: &str) -> Option<PathBuf> {
    ask_manager_with_timeout(manager, program, MANAGER_TIMEOUT)
}

/// [`ask_manager`] with the deadline exposed for tests.
fn ask_manager_with_timeout(manager: &Path, program: &str, timeout: Duration) -> Option<PathBuf> {
    let mut command = Command::new(manager);
    command
        .arg("which")
        .arg(program)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    declared_path(&run_bounded(command, timeout)?)
}

/// The first line of a successful answer, when it names an existing file.
fn declared_path(output: &Output) -> Option<PathBuf> {
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let declared = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let path = PathBuf::from(declared);
    path.is_file().then_some(path)
}

/// Wait for `command` at most `timeout`, killing it if it overruns.
fn run_bounded(mut command: Command, timeout: Duration) -> Option<Output> {
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// The per-platform directories a shellless launch does not inherit, most
/// specific first.
///
/// A caller that ships its own runtime prepends that runtime's `bin`
/// directory before this list; everything here is the user's environment.
pub fn program_directories() -> Vec<PathBuf> {
    let mut directories = all_directories(&environment);
    // A relative override would silently depend on the working directory.
    directories.retain(|directory| directory.is_absolute());
    directories
}

/// The file names one program may carry on this platform.
///
/// Windows appends the extension to the executable, so a bare `node` join
/// never found `node.exe`.
pub fn executable_names(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        vec![name.to_string(), format!("{name}.exe")]
    }
    #[cfg(not(windows))]
    {
        vec![name.to_string()]
    }
}

/// The first existing file named by `names` in `directories`, in order.
///
/// Only a regular file matches: a directory that happens to carry the
/// program's name is not spawnable, and reporting it would turn a clear
/// "not found" into a failed spawn.
pub fn find_in(directories: &[PathBuf], names: &[String]) -> Option<PathBuf> {
    for directory in directories {
        for name in names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// One environment variable as a directory, treating an empty value as absent.
fn environment(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// The complete list, with the environment lookup injected for tests.
fn all_directories(env: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = home_dir() {
        directories.extend(user_directories(&home, env));
    }
    directories.extend(system_directories(env));
    directories
}

/// The per-user program directories, in this platform's layout.
#[cfg(not(windows))]
fn user_directories(home: &Path, env: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let data_home = env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"));
    let mise = env("MISE_DATA_DIR").unwrap_or_else(|| data_home.join("mise"));
    let nvm = env("NVM_DIR").unwrap_or_else(|| home.join(".nvm"));
    let asdf = env("ASDF_DATA_DIR").unwrap_or_else(|| home.join(".asdf"));
    let fnm = env("FNM_DIR").unwrap_or_else(|| data_home.join("fnm"));

    // The versioned binary first: no shim stands between BSH and the
    // interpreter, so a version the manager only activates through the shell
    // environment still runs.
    let mut directories = Vec::new();
    directories.extend(nested_bins(&mise.join("installs").join("node"), &["bin"]));
    if let Some(selected) = nvm_selected_interpreter(&nvm) {
        directories.push(selected);
    }
    directories.extend(nested_bins(&nvm.join("versions").join("node"), &["bin"]));
    directories.extend(nested_bins(&asdf.join("installs").join("nodejs"), &["bin"]));
    directories.extend(nested_bins(
        &fnm.join("node-versions"),
        &["installation", "bin"],
    ));
    directories.push(fnm.join("aliases").join("default").join("bin"));

    // Shims and plain user bins, for an install whose layout is not above.
    directories.push(home.join(".local").join("bin"));
    directories.push(mise.join("shims"));
    directories.push(asdf.join("shims"));
    directories.push(
        env("VOLTA_HOME")
            .unwrap_or_else(|| home.join(".volta"))
            .join("bin"),
    );
    directories.push(data_home.join("pnpm"));
    if cfg!(target_os = "macos") {
        directories.push(home.join("Library").join("pnpm"));
    }
    directories
}

/// The per-user program directories, in Windows' layout.
#[cfg(windows)]
fn user_directories(home: &Path, env: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let local = env("LOCALAPPDATA");
    let app_data = env("APPDATA");
    let mise = env("MISE_DATA_DIR").or_else(|| local.as_ref().map(|dir| dir.join("mise")));
    let nvm = env("NVM_DIR")
        .or_else(|| env("NVM_HOME"))
        .or_else(|| app_data.as_ref().map(|dir| dir.join("nvm")));
    let asdf = env("ASDF_DATA_DIR").unwrap_or_else(|| home.join(".asdf"));
    let fnm = env("FNM_DIR").or_else(|| app_data.as_ref().map(|dir| dir.join("fnm")));

    // The versioned binary first: no shim stands between BSH and the
    // interpreter.
    let mut directories = Vec::new();
    if let Some(mise) = &mise {
        directories.extend(version_directories_with_bin(
            &mise.join("installs").join("node"),
        ));
    }
    if let Some(nvm) = &nvm {
        directories.extend(version_directories_with_bin(nvm));
    }
    directories.extend(version_directories_with_bin(
        &asdf.join("installs").join("nodejs"),
    ));
    if let Some(fnm) = &fnm {
        directories.extend(nested_bins(&fnm.join("node-versions"), &["installation"]));
        directories.push(fnm.join("aliases").join("default"));
    }

    // Shims and plain user bins, for an install whose layout is not above.
    directories.push(home.join("scoop").join("shims"));
    directories.push(
        home.join("scoop")
            .join("apps")
            .join("nodejs")
            .join("current"),
    );
    if let Some(mise) = &mise {
        directories.push(mise.join("shims"));
    }
    if let Some(local) = &local {
        directories.push(local.join("pnpm"));
        directories.push(local.join("Volta").join("bin"));
        directories.push(local.join("Programs").join("nodejs"));
    }
    if let Some(volta) = env("VOLTA_HOME") {
        directories.push(volta.join("bin"));
    }
    if let Some(nvm) = &nvm {
        directories.push(nvm.clone());
    }
    if let Some(link) = env("NVM_SYMLINK") {
        directories.push(link);
    }
    directories
}

/// The system-wide program directories, in this platform's layout.
#[cfg(not(windows))]
fn system_directories(_env: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if cfg!(target_os = "macos") {
        directories.push(PathBuf::from("/opt/homebrew/bin"));
    } else {
        directories.push(PathBuf::from("/home/linuxbrew/.linuxbrew/bin"));
        directories.push(PathBuf::from("/snap/bin"));
    }
    directories.push(PathBuf::from("/usr/local/bin"));
    directories.push(PathBuf::from("/usr/bin"));
    directories
}

/// The system-wide program directories, in Windows' layout.
#[cfg(windows)]
fn system_directories(env: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = env(variable) {
            directories.push(root.join("nodejs"));
        }
    }
    if let Some(program_data) = env("ProgramData") {
        directories.push(program_data.join("chocolatey").join("bin"));
    }
    directories
}

/// `<root>/<version>/<suffix...>` for every installed version, newest first.
fn nested_bins(root: &Path, suffix: &[&str]) -> Vec<PathBuf> {
    version_directories(root)
        .into_iter()
        .map(|version| suffix.iter().fold(version, |path, part| path.join(part)))
        .collect()
}

/// A Windows version directory and its `bin`, newest first.
///
/// A Windows Node distribution keeps `node.exe` at the version root, while
/// some package managers add a `bin` directory; both are probed.
#[cfg(windows)]
fn version_directories_with_bin(root: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for version in version_directories(root) {
        directories.push(version.join("bin"));
        directories.push(version);
    }
    directories
}

/// The interpreter nvm's own default alias selects, when the installed
/// versions alone can resolve it.
///
/// nvm is a shell function, so it cannot be asked like mise; its
/// `alias/default` file is the next best source of the version it would pick
/// for a launch with no shell. Remote aliases (`lts/*`, `node`) cannot be
/// resolved offline and fall through to newest-first scanning.
#[cfg(not(windows))]
fn nvm_selected_interpreter(nvm: &Path) -> Option<PathBuf> {
    let selector = std::fs::read_to_string(nvm.join("alias").join("default")).ok()?;
    let selector = selector.trim();
    if selector.is_empty() {
        return None;
    }
    let version = selected_version(&nvm.join("versions").join("node"), selector)?;
    let interpreter = version.join("bin").join("node");
    interpreter.is_file().then_some(interpreter)
}

/// The installed version directory matching `selector`, newest first.
///
/// `20` and `20.11` match by prefix; anything that is not a version number
/// (`lts/*`, `node`, `default`) has no local answer.
#[cfg(not(windows))]
fn selected_version(root: &Path, selector: &str) -> Option<PathBuf> {
    let wanted = selector.trim().trim_start_matches('v');
    if !wanted.starts_with(|character: char| character.is_ascii_digit()) {
        return None;
    }
    version_directories(root).into_iter().find(|version| {
        let name = version
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .trim_start_matches('v');
        name == wanted || name.starts_with(&format!("{wanted}."))
    })
}

/// Installed version directories under `root`, newest first.
///
/// Names that are not semantic versions (`lts`, `latest`, `lts-krypton`) sort
/// after parsed ones; they are still probed.
fn version_directories(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    versions.sort_by_key(|version| std::cmp::Reverse(version_key(version)));
    versions
}

/// Sort key for one installed version, parsed when the name carries `vX.Y.Z`.
fn version_key(path: &Path) -> Option<semver::Version> {
    let name = path.file_name()?.to_str()?.trim_start_matches('v');
    semver::Version::parse(name).ok()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::managed_interpreter_with;
    use super::{executable_names, find_in, nested_bins, program_directories, user_directories};

    /// A directory is not a program, so a `node/` directory cannot pass for
    /// the interpreter a package needs.
    #[test]
    fn only_a_file_counts_as_a_program() {
        let scratch = tempfile::tempdir().expect("scratch");
        let bin = scratch.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let names = executable_names("node");
        assert!(find_in(std::slice::from_ref(&bin), &names).is_none());
        std::fs::write(bin.join(names[0].as_str()), "#!/bin/sh\n").expect("write");
        assert_eq!(
            find_in(&[scratch.path().join("missing"), bin.clone()], &names),
            Some(bin.join(names[0].as_str()))
        );
    }

    /// The version managers keep one directory per installed Node; the newest
    /// is probed first, and a `v` prefix is not a parse failure.
    #[test]
    fn installed_versions_are_searched_newest_first() {
        let scratch = tempfile::tempdir().expect("scratch");
        for version in ["v20.11.1", "v9.11.2", "v24.15.0"] {
            std::fs::create_dir_all(scratch.path().join(version).join("bin")).expect("version");
        }
        let bins = nested_bins(scratch.path(), &["bin"]);
        assert_eq!(
            bins.first(),
            Some(&scratch.path().join("v24.15.0").join("bin"))
        );
        assert_eq!(
            bins.last(),
            Some(&scratch.path().join("v9.11.2").join("bin"))
        );
    }

    /// fnm nests the interpreter one level deeper; the suffix walk must reach
    /// it without changing the newest-first order.
    #[test]
    fn an_installation_suffix_is_walked() {
        let scratch = tempfile::tempdir().expect("scratch");
        for version in ["v20.11.1", "v22.3.0"] {
            std::fs::create_dir_all(
                scratch
                    .path()
                    .join(version)
                    .join("installation")
                    .join("bin"),
            )
            .expect("version");
        }
        let bins = nested_bins(scratch.path(), &["installation", "bin"]);
        assert_eq!(
            bins.first(),
            Some(
                &scratch
                    .path()
                    .join("v22.3.0")
                    .join("installation")
                    .join("bin")
            )
        );
    }

    /// Every candidate is absolute: a relative entry would silently depend on
    /// the launch working directory.
    #[test]
    fn the_search_directories_are_absolute() {
        assert!(
            program_directories()
                .iter()
                .all(|directory| directory.is_absolute()),
            "a program directory must not depend on the working directory"
        );
    }

    /// A mise install must be used through its versioned binary, not through
    /// the shim: the shim resolves the version from the manager's own
    /// configuration and refuses outside a shell, while the binary it would
    /// have run is directly spawnable.
    #[cfg(not(windows))]
    #[test]
    fn a_versioned_binary_wins_over_its_shim() {
        let scratch = tempfile::tempdir().expect("scratch");
        let home = scratch.path();
        let install = home
            .join(".local")
            .join("share")
            .join("mise")
            .join("installs")
            .join("node")
            .join("v24.15.0")
            .join("bin");
        let shims = home.join(".local").join("share").join("mise").join("shims");
        std::fs::create_dir_all(&install).expect("install");
        std::fs::create_dir_all(&shims).expect("shims");
        std::fs::write(install.join("node"), "#!/bin/sh\n").expect("install node");
        std::fs::write(shims.join("node"), "#!/bin/sh\n").expect("shim node");

        let env = |_: &str| None;
        let directories = user_directories(home, &env);
        assert_eq!(
            find_in(&directories, &executable_names("node")),
            Some(install.join("node")),
            "the shim must not shadow the versioned binary"
        );
    }

    /// A manager that keeps its data outside the default home directory (a
    /// custom mise or nvm store) is found through its own environment
    /// variable, exactly as the manager itself would find it.
    #[cfg(not(windows))]
    #[test]
    fn a_version_manager_data_directory_override_is_honoured() {
        let scratch = tempfile::tempdir().expect("scratch");
        let home = scratch.path();
        let custom = scratch.path().join("custom-mise");
        let install = custom
            .join("installs")
            .join("node")
            .join("v22.1.0")
            .join("bin");
        std::fs::create_dir_all(&install).expect("install");
        std::fs::write(install.join("node"), "#!/bin/sh\n").expect("node");

        let env = {
            let custom = custom.clone();
            move |name: &str| (name == "MISE_DATA_DIR").then(|| custom.clone())
        };
        let directories = user_directories(home, &env);
        assert_eq!(
            find_in(&directories, &executable_names("node")),
            Some(install.join("node"))
        );
    }

    /// The XDG data home relocates mise, fnm and pnpm together, so the probe
    /// follows it instead of assuming `~/.local/share`.
    #[cfg(not(windows))]
    #[test]
    fn xdg_data_home_is_honoured() {
        let scratch = tempfile::tempdir().expect("scratch");
        let home = scratch.path();
        let data_home = scratch.path().join("xdg-data");
        let install = data_home
            .join("mise")
            .join("installs")
            .join("node")
            .join("v21.7.0")
            .join("bin");
        std::fs::create_dir_all(&install).expect("install");
        std::fs::write(install.join("node"), "#!/bin/sh\n").expect("node");

        let env = {
            let data_home = data_home.clone();
            move |name: &str| (name == "XDG_DATA_HOME").then(|| data_home.clone())
        };
        let directories = user_directories(home, &env);
        assert_eq!(
            find_in(&directories, &executable_names("node")),
            Some(install.join("node"))
        );
    }

    /// nvm's data directory is honoured the same way, so an nvm install works
    /// without the shell that normally defines `NVM_DIR`.
    #[cfg(not(windows))]
    #[test]
    fn nvm_dir_is_honoured() {
        let scratch = tempfile::tempdir().expect("scratch");
        let home = scratch.path();
        let nvm = scratch.path().join("nvm");
        let install = nvm
            .join("versions")
            .join("node")
            .join("v20.11.1")
            .join("bin");
        std::fs::create_dir_all(&install).expect("install");
        std::fs::write(install.join("node"), "#!/bin/sh\n").expect("node");

        let env = {
            let nvm = nvm.clone();
            move |name: &str| (name == "NVM_DIR").then(|| nvm.clone())
        };
        let directories = user_directories(home, &env);
        assert_eq!(
            find_in(&directories, &executable_names("node")),
            Some(install.join("node"))
        );
    }

    /// A mock manager for the execution-seam tests: a shell script that runs
    /// `body` when asked `which <program>`.
    #[cfg(unix)]
    fn manager_script(directory: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::create_dir_all(directory).expect("manager dir");
        let manager = directory.join("mise");
        std::fs::write(&manager, format!("#!/bin/sh\n{body}\n")).expect("manager script");
        let mut permissions = std::fs::metadata(&manager).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&manager, permissions).expect("chmod");
        manager
    }

    /// A manager that answers `which` owns the choice: this is how a mise
    /// selection (global, project or environment) stays effective in an app
    /// launched without a shell.
    #[cfg(unix)]
    #[test]
    fn a_manager_selection_wins() {
        let scratch = tempfile::tempdir().expect("scratch");
        let node = scratch.path().join("chosen-node");
        std::fs::write(&node, "#!/bin/sh\n").expect("node");
        let manager_dir = scratch.path().join("bin");
        manager_script(
            &manager_dir,
            &format!("printf '%s\\n' '{}'", node.display()),
        );

        let env = {
            let manager_dir = manager_dir.clone();
            move |name: &str| (name == "PATH").then(|| manager_dir.clone())
        };
        assert_eq!(managed_interpreter_with("node", &env), Some(node));
    }

    /// A manager that fails is not a selection: the caller keeps searching
    /// instead of spawning a broken path.
    #[cfg(unix)]
    #[test]
    fn a_failing_manager_falls_through() {
        let scratch = tempfile::tempdir().expect("scratch");
        let manager_dir = scratch.path().join("bin");
        manager_script(&manager_dir, "exit 1");
        let env = {
            let manager_dir = manager_dir.clone();
            move |name: &str| (name == "PATH").then(|| manager_dir.clone())
        };
        assert_eq!(managed_interpreter_with("node", &env), None);
    }

    /// A manager that never answers must not hang the extension host.
    #[cfg(unix)]
    #[test]
    fn a_silent_manager_is_bounded() {
        let scratch = tempfile::tempdir().expect("scratch");
        let manager = manager_script(&scratch.path().join("bin"), "sleep 5");
        let started = std::time::Instant::now();
        assert!(super::ask_manager_with_timeout(
            &manager,
            "node",
            std::time::Duration::from_millis(150)
        )
        .is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }

    /// nvm's default alias is effective when it names a locally installed
    /// version; a remote alias has no offline answer and falls through.
    #[cfg(not(windows))]
    #[test]
    fn nvm_default_alias_selects_its_version() {
        let scratch = tempfile::tempdir().expect("scratch");
        let nvm = scratch.path().join("nvm");
        for version in ["v18.20.4", "v20.11.1", "v22.1.0"] {
            let bin = nvm.join("versions").join("node").join(version).join("bin");
            std::fs::create_dir_all(&bin).expect("version");
            std::fs::write(bin.join("node"), "#!/bin/sh\n").expect("node");
        }
        let alias = nvm.join("alias");
        std::fs::create_dir_all(&alias).expect("alias");

        std::fs::write(alias.join("default"), "20\n").expect("alias");
        assert_eq!(
            super::nvm_selected_interpreter(&nvm),
            Some(
                nvm.join("versions")
                    .join("node")
                    .join("v20.11.1")
                    .join("bin")
                    .join("node")
            )
        );

        std::fs::write(alias.join("default"), "lts/*\n").expect("alias");
        assert!(super::nvm_selected_interpreter(&nvm).is_none());
    }

    /// Windows nvm keeps `node.exe` at the version root; the probe must look
    /// there, not only in `bin`.
    #[cfg(windows)]
    #[test]
    fn nvm_home_is_honoured_on_windows() {
        let scratch = tempfile::tempdir().expect("scratch");
        let home = scratch.path();
        let nvm = scratch.path().join("nvm");
        let version = nvm.join("v22.1.0");
        std::fs::create_dir_all(&version).expect("version");
        std::fs::write(version.join("node.exe"), "").expect("node.exe");

        let env = {
            let nvm = nvm.clone();
            move |name: &str| (name == "NVM_HOME").then(|| nvm.clone())
        };
        let directories = user_directories(home, &env);
        assert!(directories.contains(&version));
    }
}
