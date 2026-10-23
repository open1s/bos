//! Put a pane's shell on the terminal the sandbox opened for it.
//!
//! The sandbox spawn opens the pane's tty by path, so the shell is never a
//! session leader and never owns a controlling terminal: fish refuses to run
//! (`tcgetpgrp failed`), bash disables job control, and foreground signals have
//! no group to reach.
//!
//! The helper forks first. The sandbox can leave its child as a process-group
//! leader, and `setsid` refuses to run in one; the child therefore becomes the
//! session leader on the tty while the parent stays as the process the pane
//! tracks, mirrors the shell's exit status, and forwards termination signals so
//! a killed pane never leaves a shell holding the pty.
//!
//! Usage: `bsh-tty-exec <tty> <program> [args...]`

#[cfg(unix)]
static CHILD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

#[cfg(unix)]
extern "C" fn forward(signal: libc::c_int) {
    let pid = CHILD.load(std::sync::atomic::Ordering::Relaxed);
    if pid > 1 {
        unsafe {
            libc::kill(pid, signal);
        }
    }
}

#[cfg(unix)]
fn main() {
    use std::os::unix::io::AsRawFd;
    use std::os::unix::process::CommandExt as _;

    let mut args = std::env::args_os();
    let _ = args.next();
    let (Some(tty), Some(program)) = (args.next(), args.next()) else {
        fail("usage: bsh-tty-exec <tty> <program> [args...]");
    };
    let rest: Vec<_> = args.collect();
    let tty = std::path::PathBuf::from(tty);
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&tty)
    {
        Ok(file) => file,
        Err(error) => fail(&format!("open {}: {error}", tty.display())),
    };
    let fd = file.as_raw_fd();

    match unsafe { libc::fork() } {
        // The child is not a group leader, so the session and the controlling
        // terminal can actually be claimed before the shell starts.
        0 => {
            unsafe {
                libc::setsid();
                libc::ioctl(fd, libc::TIOCSCTTY as libc::c_ulong, 0);
            }
            for target in 0..3 {
                if unsafe { libc::dup2(fd, target) } < 0 {
                    fail("dup2 onto the pane's tty failed");
                }
            }
            drop(file);
            let error = std::process::Command::new(&program).args(&rest).exec();
            fail(&format!("exec {}: {error}", program.display()));
        }
        pid if pid > 0 => {
            CHILD.store(pid, std::sync::atomic::Ordering::Relaxed);
            for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT] {
                unsafe {
                    libc::signal(signal, forward as *const () as usize as libc::sighandler_t);
                }
            }
            let mut status = 0;
            loop {
                let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
                if waited == pid {
                    break;
                }
                if waited < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
                {
                    continue;
                }
                break;
            }
            let code = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else {
                128 + libc::WTERMSIG(status)
            };
            std::process::exit(code);
        }
        _ => fail("fork failed"),
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("bsh-tty-exec: only Unix terminals need the login_tty handshake");
    std::process::exit(2);
}

#[cfg(unix)]
fn fail(message: &str) -> ! {
    eprintln!("bsh-tty-exec: {message}");
    std::process::exit(1);
}
