//! Intelligent review: exact change capture, structural diffs and decisions.
//!
//! The module is independent of GPUI: tools write snapshots through
//! [store::ReviewRuntime], the store persists them under .bsh/review, and the
//! shell reads a prepared [domain::ReviewSnapshot].

pub(crate) mod diff;
pub(crate) mod domain;
pub(crate) mod store;

pub(crate) use store::ReviewRuntime;
