//! Where an app finds user-installed programs without a shell `PATH`.
//!
//! A GUI application does not inherit the shell environment: a Finder-launched
//! macOS app gets launchd's minimal path, and a process started from Explorer
//! on Windows gets only the machine variables. The version managers (mise,
//! nvm, asdf, pyenv, fnm, uv, Volta, conda) keep their programs in per-user
//! directories a shellless launch never sees, so every runtime that resolves
//! an external program would otherwise repeat - and drift on - the same
//! platform list.
//!
//! [`executable`] is that one resolution, for any program name:
//!
//! 1. `$BOS_<PROGRAM>` (for example `BOS_PYTHON3`, `BOS_NODE`), when it names
//!    an existing file.
//! 2. An activated virtual environment (`VIRTUAL_ENV`).
//! 3. The manager's own selection: `mise`, `asdf`, `pyenv` and `volta` answer
//!    `which <program>` (bounded, and only when they hold a shim for it).
//! 4. The managers' installed versions, shims, `PATH` and the system prefixes.
//!
//! Asking the manager is what keeps its configuration - global, project or
//! environment - in charge instead of guessing the newest installed version.
//! The query is bounded and its output must name an existing file; a manager
//! that is absent, fails, or does not answer in time falls through to the
//! directory scan.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::home::home_dir;

/// How long a version manager may take to name its program.
const MANAGER_TIMEOUT: Duration = Duration::from_millis(1500);

/// The managers that answer `which <program>` and keep shims per program.
const MANAGERS: &[&str] = &["mise", "asdf", "pyenv", "volta"];

/// Resolve one program name to an executable, cross-platform.
///
/// The search is a filesystem probe except for one bounded question to an
/// installed version manager, which is the only party that knows which version
/// its own configuration selects. A missing program returns `None`; the caller
/// then refuses the spawn with a clear reason instead of guessing.
pub fn executable(name: &str) -> Option<PathBuf> {
    executable_with(name, &environment)
}

/// The environment override variable for one program name.
///
/// `python3` becomes `BOS_PYTHON3`; every non-alphanumeric character becomes
/// an underscore, so `bunx` is `BOS_BUNX`, and a dotted name never reaches an
/// unrepresentable variable.
fn override_variable(name: &str) -> String {
    let mut variable = String::from("BOS_");
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            variable.push(character.to_ascii_uppercase());
        } else {
            variable.push('_');
        }
    }
    variable
}

/// [`executable`] with the environment lookup injected for tests.
fn executable_with(name: &str, env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let names = executable_names(name);
    if let Some(explicit) = env(&override_variable(name)) {
        if explicit.is_file() {
            return Some(explicit);
        }
    }
    if let Some(venv) = env("VIRTUAL_ENV") {
        #[cfg(windows)]
        let bin = venv.join("Scripts");
        #[cfg(not(windows))]
        let bin = venv.join("bin");
        if let Some(path) = find_in(&[bin], &names) {
            return Some(path);
        }
    }
    if let Some(path) = manager_selection(name, env) {
        return Some(path);
    }
    find_in(&program_directories_with(env), &names)
}

/// Ask each manager that holds a shim for `program` which one it selects.
///
/// The shim is the cheap gate: a manager without one has nothing to say about
/// this program, so the process is never spawned for it.
fn manager_selection(program: &str, env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    for manager in MANAGERS {
        if !manager_offers(manager, program, env) {
            continue;
        }
        let Some(executable) = find_manager(manager, env) else {
            continue;
        };
        if let Some(path) = ask_manager(&executable, program) {
            return Some(path);
        }
    }
    None
}

/// Whether the manager keeps a shim named after `program`.
fn manager_offers(manager: &str, program: &str, env: &dyn Fn(&str) -> Option<PathBuf>) -> bool {
    let Some(shims) = manager_shims(manager, env) else {
        return false;
    };
    find_in(&[shims], &executable_names(program)).is_some()
}

/// The directory a manager writes one shim per managed program into.
fn manager_shims(manager: &str, env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let roots = Roots::discover(env)?;
    Some(match manager {
        "mise" => roots.mise.join("shims"),
        "asdf" => roots.asdf.join("shims"),
        "pyenv" => roots.pyenv.join("shims"),
        "volta" => roots.volta.join("bin"),
        _ => return None,
    })
}

/// The manager executable itself, found the same way a program is.
fn find_manager(name: &str, env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let mut directories = path_directories(env);
    directories.extend(managed_directories(env));
    find_in(&directories, &executable_names(name))
}

/// Ask one manager, bounded, and accept only an existing file.
fn ask_manager(executable: &Path, program: &str) -> Option<PathBuf> {
    ask_manager_with_timeout(executable, program, MANAGER_TIMEOUT)
}

/// [`ask_manager`] with the deadline exposed for tests.
fn ask_manager_with_timeout(
    executable: &Path,
    program: &str,
    timeout: Duration,
) -> Option<PathBuf> {
    let mut command = Command::new(executable);
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
/// directory before this list; everything here is the user's environment and
/// `PATH`.
pub fn program_directories() -> Vec<PathBuf> {
    program_directories_with(&environment)
}

/// [`program_directories`] with the environment lookup injected for tests.
fn program_directories_with(env: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let mut directories = managed_directories(env);
    directories.extend(path_directories(env));
    directories.extend(system_directories(env));
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

/// The `PATH` entries, in order.
fn path_directories(env: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    env("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

/// The per-user program directories this module knows by manager convention.
#[cfg(not(windows))]
fn managed_directories(env: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let Some(roots) = Roots::discover(env) else {
        return Vec::new();
    };
    let mut directories = Vec::new();
    // Installed versions first: no shim stands between the host and the
    // interpreter, so a version the manager only activates through the shell
    // environment still runs.
    directories.extend(tool_bins(&roots.mise.join("installs")));
    directories.extend(tool_bins(&roots.asdf.join("installs")));
    directories.extend(version_bins(&roots.pyenv.join("versions")));
    if let Some(selected) = nvm_selected_bin(&roots.nvm) {
        directories.push(selected);
    }
    directories.extend(nested_bins(
        &roots.nvm.join("versions").join("node"),
        &["bin"],
    ));
    directories.extend(nested_bins(
        &roots.fnm.join("node-versions"),
        &["installation", "bin"],
    ));
    directories.extend(version_bins(&roots.uv));
    directories.extend(tool_bins(&roots.volta.join("tools").join("image")));
    if let Some(conda) = env("CONDA_PREFIX") {
        directories.push(conda.join("bin"));
    }
    for base in ["miniconda3", "anaconda3", "miniforge3"] {
        directories.push(roots.home.join(base).join("bin"));
    }
    // Shims and plain user bins, for an install whose layout is not above.
    directories.push(roots.home.join(".local").join("bin"));
    directories.push(roots.mise.join("shims"));
    directories.push(roots.asdf.join("shims"));
    directories.push(roots.pyenv.join("shims"));
    directories.push(roots.pyenv.join("bin"));
    directories.push(roots.asdf.join("bin"));
    directories.push(roots.volta.join("bin"));
    directories.push(roots.data_home.join("pnpm"));
    if cfg!(target_os = "macos") {
        directories.push(roots.home.join("Library").join("pnpm"));
    }
    directories
}

/// The per-user program directories this module knows by manager convention.
#[cfg(windows)]
fn managed_directories(env: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let Some(roots) = Roots::discover(env) else {
        return Vec::new();
    };
    let mut directories = Vec::new();
    // Installed versions first; a Windows tool keeps its executable at the
    // version root or in `bin`, so both are probed.
    directories.extend(tool_versions_with_bin(&roots.mise.join("installs")));
    directories.extend(tool_versions_with_bin(&roots.asdf.join("installs")));
    directories.extend(version_directories_with_bin(&roots.pyenv.join("versions")));
    directories.extend(version_directories_with_bin(&roots.nvm));
    directories.extend(nested_bins(
        &roots.fnm.join("node-versions"),
        &["installation"],
    ));
    directories.extend(version_directories_with_bin(&roots.uv));
    directories.extend(tool_versions_with_bin(
        &roots.volta.join("tools").join("image"),
    ));
    directories.extend(version_directories_with_bin(
        &roots.local.join("Programs").join("Python"),
    ));
    directories.push(roots.local.join("Programs").join("nodejs"));
    directories.extend(tool_current_bins(&roots.home.join("scoop").join("apps")));
    // Shims and plain user bins.
    directories.push(roots.home.join("scoop").join("shims"));
    directories.push(roots.mise.join("shims"));
    directories.push(roots.asdf.join("shims"));
    directories.push(roots.pyenv.join("shims"));
    directories.push(roots.volta.join("bin"));
    directories.push(roots.local.join("pnpm"));
    if let Some(conda) = env("CONDA_PREFIX") {
        directories.push(conda);
    }
    for base in ["miniconda3", "anaconda3", "miniforge3"] {
        directories.push(roots.home.join(base));
    }
    directories
}

/// One platform's version-manager data roots.
#[cfg(not(windows))]
struct Roots {
    home: PathBuf,
    data_home: PathBuf,
    mise: PathBuf,
    asdf: PathBuf,
    nvm: PathBuf,
    fnm: PathBuf,
    pyenv: PathBuf,
    uv: PathBuf,
    volta: PathBuf,
}

#[cfg(not(windows))]
impl Roots {
    fn discover(env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<Self> {
        let home = home_dir()?;
        let data_home = env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"));
        Some(Self {
            mise: env("MISE_DATA_DIR").unwrap_or_else(|| data_home.join("mise")),
            asdf: env("ASDF_DATA_DIR").unwrap_or_else(|| home.join(".asdf")),
            nvm: env("NVM_DIR").unwrap_or_else(|| home.join(".nvm")),
            fnm: env("FNM_DIR").unwrap_or_else(|| data_home.join("fnm")),
            pyenv: env("PYENV_ROOT").unwrap_or_else(|| home.join(".pyenv")),
            uv: env("UV_PYTHON_INSTALL_DIR").unwrap_or_else(|| data_home.join("uv").join("python")),
            volta: env("VOLTA_HOME").unwrap_or_else(|| home.join(".volta")),
            home,
            data_home,
        })
    }
}

/// One platform's version-manager data roots.
#[cfg(windows)]
struct Roots {
    home: PathBuf,
    local: PathBuf,
    mise: PathBuf,
    asdf: PathBuf,
    nvm: PathBuf,
    fnm: PathBuf,
    pyenv: PathBuf,
    uv: PathBuf,
    volta: PathBuf,
}

#[cfg(windows)]
impl Roots {
    fn discover(env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<Self> {
        let home = home_dir()?;
        let local = env("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"));
        let app_data = env("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"));
        Some(Self {
            mise: env("MISE_DATA_DIR").unwrap_or_else(|| local.join("mise")),
            asdf: env("ASDF_DATA_DIR").unwrap_or_else(|| home.join(".asdf")),
            nvm: env("NVM_DIR")
                .or_else(|| env("NVM_HOME"))
                .unwrap_or_else(|| app_data.join("nvm")),
            fnm: env("FNM_DIR").unwrap_or_else(|| app_data.join("fnm")),
            pyenv: env("PYENV_ROOT").unwrap_or_else(|| home.join(".pyenv").join("pyenv-win")),
            uv: env("UV_PYTHON_INSTALL_DIR").unwrap_or_else(|| local.join("uv").join("python")),
            volta: env("VOLTA_HOME").unwrap_or_else(|| local.join("Volta")),
            home,
            local,
        })
    }
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

/// The `bin` directory of each tool and version under `root`.
#[cfg(not(windows))]
fn tool_bins(root: &Path) -> Vec<PathBuf> {
    directories(root)
        .into_iter()
        .flat_map(|tool| version_bins(&tool))
        .collect()
}

/// Each tool's version directory and its `bin`, under `root`.
#[cfg(windows)]
fn tool_versions_with_bin(root: &Path) -> Vec<PathBuf> {
    directories(root)
        .into_iter()
        .flat_map(|tool| version_directories_with_bin(&tool))
        .collect()
}

/// Each tool's `current` directory and its `bin`, under `root`.
#[cfg(windows)]
fn tool_current_bins(root: &Path) -> Vec<PathBuf> {
    directories(root)
        .into_iter()
        .flat_map(|tool| {
            let current = tool.join("current");
            [current.join("bin"), current]
        })
        .collect()
}

/// The `bin` directory under each installed version, newest first.
#[cfg(not(windows))]
fn version_bins(root: &Path) -> Vec<PathBuf> {
    nested_bins(root, &["bin"])
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
/// A Windows tool distribution keeps its executable at the version root,
/// while some package managers add a `bin` directory; both are probed.
#[cfg(windows)]
fn version_directories_with_bin(root: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for version in version_directories(root) {
        directories.push(version.join("bin"));
        directories.push(version);
    }
    directories
}

/// The immediate subdirectories of `root`, in name order.
fn directories(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut directories: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();
    directories
}

/// Installed version directories under `root`, newest first.
///
/// Names that are not semantic versions (`lts`, `latest`, `lts-krypton`) sort
/// after parsed ones; they are still probed.
fn version_directories(root: &Path) -> Vec<PathBuf> {
    let mut versions = directories(root);
    versions.sort_by_key(|version| std::cmp::Reverse(version_key(version)));
    versions
}

/// Sort key for one installed version.
///
/// Accepts `vX.Y.Z`, a bare `X.Y.Z`, and a distribution directory that
/// prefixes the number (`cpython-3.12.11-macos-aarch64-none`). A name with no
/// number (`lts`, `latest`, `lts-krypton`) sorts after parsed ones and is
/// still probed.
fn version_key(path: &Path) -> Option<semver::Version> {
    let name = path.file_name()?.to_str()?.trim_start_matches('v');
    if let Ok(version) = semver::Version::parse(name) {
        return Some(version);
    }
    let start = name.find(|character: char| character.is_ascii_digit())?;
    let number = &name[start..];
    let end = number
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(number.len());
    let candidate = &number[..end];
    let mut parts: Vec<&str> = candidate.split('.').collect();
    while parts.len() < 3 {
        parts.push("0");
    }
    if parts.len() > 3 {
        return None;
    }
    semver::Version::parse(&parts.join(".")).ok()
}

/// The `bin` directory nvm's own default alias selects, when the installed
/// versions alone can resolve it.
///
/// nvm is a shell function, so it cannot be asked like mise; its
/// `alias/default` file is the next best source of the version it would pick
/// for a launch with no shell. Remote aliases (`lts/*`, `node`) cannot be
/// resolved offline and fall through to newest-first scanning.
#[cfg(not(windows))]
fn nvm_selected_bin(nvm: &Path) -> Option<PathBuf> {
    let selector = std::fs::read_to_string(nvm.join("alias").join("default")).ok()?;
    let selector = selector.trim();
    if selector.is_empty() {
        return None;
    }
    let version = selected_version(&nvm.join("versions").join("node"), selector)?;
    Some(version.join("bin"))
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

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    use super::nvm_selected_bin;
    use super::{
        executable_names, executable_with, find_in, nested_bins, override_variable,
        program_directories,
    };
    use std::path::{Path, PathBuf};

    /// Write a file (and its parents) with the given contents.
    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parents");
        }
        std::fs::write(path, contents).expect("write");
    }

    /// Write an executable shell script, the shape every manager mock takes.
    #[cfg(unix)]
    fn script(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt as _;
        write(path, &format!("#!/bin/sh\n{body}\n"));
        let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }

    /// An environment lookup over a fixed set of variables.
    fn environment_from(pairs: Vec<(&'static str, PathBuf)>) -> impl Fn(&str) -> Option<PathBuf> {
        move |name| {
            pairs
                .iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| value.clone())
        }
    }

    /// A directory is not a program, so a `node/` directory cannot pass for
    /// the interpreter a package needs.
    #[test]
    fn only_a_file_counts_as_a_program() {
        let scratch = tempfile::tempdir().expect("scratch");
        let bin = scratch.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let names = executable_names("node");
        assert!(find_in(std::slice::from_ref(&bin), &names).is_none());
        write(&bin.join(&names[0]), "#!/bin/sh\n");
        assert_eq!(
            find_in(&[scratch.path().join("missing"), bin.clone()], &names),
            Some(bin.join(&names[0]))
        );
    }

    /// The version managers keep one directory per installed version; the
    /// newest is probed first, and a `v` prefix is not a parse failure.
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

    /// A distribution directory that prefixes the version (`cpython-3.12.11-`)
    /// still sorts by its number, so the newest installed interpreter wins.
    #[test]
    fn a_prefixed_distribution_version_is_parsed() {
        let scratch = tempfile::tempdir().expect("scratch");
        for name in [
            "cpython-3.10-macos-aarch64-none",
            "cpython-3.12.11-macos-aarch64-none",
            "cpython-3.12-macos-aarch64-none",
        ] {
            std::fs::create_dir_all(scratch.path().join(name).join("bin")).expect("version");
        }
        let bins = nested_bins(scratch.path(), &["bin"]);
        assert_eq!(
            bins.first(),
            Some(
                &scratch
                    .path()
                    .join("cpython-3.12.11-macos-aarch64-none")
                    .join("bin")
            )
        );
    }

    /// A manager that nests its interpreter one level deeper must still be
    /// walked, without losing the newest-first order.
    #[test]
    fn a_nested_installation_suffix_is_walked() {
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

    /// The search never names another application's bundle: that launcher
    /// needs an environment only the other app provides (status 127).
    #[test]
    fn the_search_directories_belong_to_this_host() {
        assert!(
            program_directories()
                .iter()
                .all(|directory| !directory.to_string_lossy().contains("DeepSeek Harness")),
            "another application's bundle is not this host's runtime"
        );
    }

    /// The override variable is derived from the program name, so every
    /// program has one without a per-program table.
    #[test]
    fn override_variables_are_derived_from_the_name() {
        assert_eq!(override_variable("node"), "BOS_NODE");
        assert_eq!(override_variable("python3"), "BOS_PYTHON3");
        assert_eq!(override_variable("bunx"), "BOS_BUNX");
    }

    /// The explicit override is the reader's decision, before any manager.
    #[test]
    fn an_explicit_override_wins() {
        let scratch = tempfile::tempdir().expect("scratch");
        let chosen = scratch.path().join("python3");
        write(&chosen, "#!/bin/sh\n");
        let env = environment_from(vec![("BOS_PYTHON3", chosen.clone())]);
        assert_eq!(executable_with("python3", &env), Some(chosen));
    }

    /// An activated virtual environment wins next: a shell that exported
    /// `VIRTUAL_ENV` must not be overridden by a manager's global version.
    #[cfg(unix)]
    #[test]
    fn an_activated_virtual_environment_wins() {
        let scratch = tempfile::tempdir().expect("scratch");
        let venv = scratch.path().join("venv");
        let bin = venv.join("bin");
        write(&bin.join("python3"), "#!/bin/sh\n");
        let env = environment_from(vec![("VIRTUAL_ENV", venv)]);
        assert_eq!(executable_with("python3", &env), Some(bin.join("python3")));
    }

    /// A manager that answers `which` owns the choice: this is how a mise
    /// selection (global, project or environment) stays effective in an app
    /// launched without a shell.
    #[cfg(unix)]
    #[test]
    fn a_manager_selection_wins() {
        let scratch = tempfile::tempdir().expect("scratch");
        let chosen = scratch.path().join("chosen-node");
        write(&chosen, "#!/bin/sh\n");
        let manager_dir = scratch.path().join("bin");
        script(
            &manager_dir.join("mise"),
            &format!("printf '%s\\n' '{}'", chosen.display()),
        );
        let data = scratch.path().join("mise-data");
        write(&data.join("shims").join("node"), "#!/bin/sh\n");

        let env = environment_from(vec![("PATH", manager_dir), ("MISE_DATA_DIR", data)]);
        assert_eq!(executable_with("node", &env), Some(chosen));
    }

    /// A manager without a shim for the program has nothing to say about it,
    /// so it is never spawned: the installed version is used instead.
    #[cfg(unix)]
    #[test]
    fn a_manager_without_a_shim_is_not_asked() {
        let scratch = tempfile::tempdir().expect("scratch");
        let manager_choice = scratch.path().join("manager-node");
        write(&manager_choice, "#!/bin/sh\n");
        let manager_dir = scratch.path().join("bin");
        script(
            &manager_dir.join("mise"),
            &format!("printf '%s\\n' '{}'", manager_choice.display()),
        );
        let data = scratch.path().join("mise-data");
        let installed = data
            .join("installs")
            .join("node")
            .join("v24.15.0")
            .join("bin")
            .join("node");
        write(&installed, "#!/bin/sh\n");

        let env = environment_from(vec![("PATH", manager_dir), ("MISE_DATA_DIR", data)]);
        assert_eq!(executable_with("node", &env), Some(installed));
    }

    /// A manager that never answers must not hang the extension host.
    #[cfg(unix)]
    #[test]
    fn a_silent_manager_is_bounded() {
        let scratch = tempfile::tempdir().expect("scratch");
        let manager = scratch.path().join("bin").join("mise");
        script(&manager, "sleep 5");
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
            write(&bin.join("node"), "#!/bin/sh\n");
        }
        let alias = nvm.join("alias");
        std::fs::create_dir_all(&alias).expect("alias");

        std::fs::write(alias.join("default"), "20\n").expect("alias");
        assert_eq!(
            nvm_selected_bin(&nvm),
            Some(
                nvm.join("versions")
                    .join("node")
                    .join("v20.11.1")
                    .join("bin")
            )
        );

        std::fs::write(alias.join("default"), "lts/*\n").expect("alias");
        assert!(nvm_selected_bin(&nvm).is_none());
    }

    /// nvm's data directory is honoured through its own variable, so an nvm
    /// install works without the shell that normally defines `NVM_DIR`.
    #[cfg(not(windows))]
    #[test]
    fn nvm_dir_is_honoured() {
        let scratch = tempfile::tempdir().expect("scratch");
        let nvm = scratch.path().join("nvm");
        let node = nvm
            .join("versions")
            .join("node")
            .join("v20.11.1")
            .join("bin")
            .join("node");
        write(&node, "#!/bin/sh\n");

        let env = environment_from(vec![
            ("NVM_DIR", nvm),
            ("MISE_DATA_DIR", scratch.path().join("empty-mise")),
        ]);
        assert_eq!(executable_with("node", &env), Some(node));
    }

    /// The XDG data home relocates mise, fnm and pnpm together, so the probe
    /// follows it instead of assuming `~/.local/share`.
    #[cfg(not(windows))]
    #[test]
    fn xdg_data_home_is_honoured() {
        let scratch = tempfile::tempdir().expect("scratch");
        let data_home = scratch.path().join("xdg-data");
        let node = data_home
            .join("mise")
            .join("installs")
            .join("node")
            .join("v21.7.0")
            .join("bin")
            .join("node");
        write(&node, "#!/bin/sh\n");

        let env = environment_from(vec![("XDG_DATA_HOME", data_home)]);
        assert_eq!(executable_with("node", &env), Some(node));
    }

    /// Windows nvm keeps `node.exe` at the version root; the probe must look
    /// there, not only in `bin`.
    #[cfg(windows)]
    #[test]
    fn nvm_home_is_honoured_on_windows() {
        let scratch = tempfile::tempdir().expect("scratch");
        let nvm = scratch.path().join("nvm");
        let node = nvm.join("v22.1.0").join("node.exe");
        write(&node, "");

        let env = environment_from(vec![
            ("NVM_HOME", nvm),
            ("MISE_DATA_DIR", scratch.path().join("empty-mise")),
        ]);
        assert_eq!(executable_with("node", &env), Some(node));
    }
}
