//! Concrete resource implementations for each of the six classes.
//!
//! `file` (storage), `folder` (storage), `sock` (network), `proc` (compute),
//! `mem` (abstract), and `combine` (combine/super virtual) are implemented
//! as `ResourceHandler`s.

pub mod combine;
pub mod file;
pub mod folder;
pub mod lock;
pub mod mem;
pub mod proc;
pub mod sock;
pub mod vnode;
pub mod watch;