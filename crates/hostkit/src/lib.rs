//! Host-neutral OS services shared by every BOS crate that touches the
//! operating system: process-tree ownership, path safety, environment
//! baselines, private files and home-directory resolution.
//!
//! The crate exists so the platform contract is written ONCE. Before it,
//! each runtime carried its own `#[cfg(unix)]` process-group code and a
//! `#[cfg(not(unix))]` no-op, which compiled everywhere but only worked on
//! Unix (issues #312, #315, #316, #320). Every process-producing runtime now
//! spawns through [`proc`], every untrusted package path through [`paths`],
//! every child environment through [`env`], and every secret file through
//! [`fspriv`].
//!
//! # Platform contract
//!
//! - Unix: a spawned process leads its own process group; termination signals
//!   the group and confirms it is empty before reporting success.
//! - Windows: a spawned process is assigned to a Job Object with
//!   kill-on-close BEFORE it can run (created suspended, assigned, resumed),
//!   so no descendant can escape the ownership window; termination kills the
//!   job and confirms the job's process list is empty.
//! - A termination that cannot be confirmed is reported as unconfirmed; a
//!   caller must never publish "killed" on an unconfirmed termination.

#![warn(missing_docs)]

pub mod env;
pub mod fspriv;
pub mod home;
pub mod paths;
pub mod proc;
