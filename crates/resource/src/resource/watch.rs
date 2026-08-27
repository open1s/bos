//! Filesystem watch (change notifications) for `file://` and `folder://`.
//!
//! Uses `notify`'s `RecommendedWatcher`. Events are bridged into the async
//! world via an unbounded Tokio channel (`send` never blocks, so the watcher
//! callback is safe to call from a sync thread), then re-exposed as a stream
//! of [`ResourceEvent`].

use std::path::{Path, PathBuf};
use std::pin::Pin;

use futures::Stream;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::action::ResourceEvent;
use crate::error::{ResourceError, Result};

/// Start watching `path` and return a stream of resource events. The watcher
/// is dropped when the stream is dropped, ending notification.
pub fn watch(path: &Path) -> Result<Pin<Box<dyn Stream<Item = ResourceEvent> + Send>>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(
        move |ev| {
            // Channel is unbounded; send never blocks or panics on a closed
            // receiver — it just fails, which is fine when the subscriber is
            // gone (the watcher thread exits shortly after).
            let _ = tx.send(ev);
        },
        notify::Config::default(),
    )
    .map_err(|e| ResourceError::Io(std::io::Error::other(e)))?;
    watcher
        .watch(path, RecursiveMode::Recursive)
        .map_err(|e| ResourceError::Io(std::io::Error::other(e)))?;

    // Map raw notify events into the unified event envelope.
    let stream = async_stream::stream! {
        // Keep the watcher alive for the lifetime of the stream; dropping it
        // unregisters the watch.
        let _watcher = watcher;
        let mut rx = rx;
        while let Some(item) = rx.recv().await {
            match item {
                Ok(Event { kind, paths, .. }) => {
                    let tag = match kind {
                        EventKind::Create(_) => "create",
                        EventKind::Modify(_) => "modify",
                        EventKind::Remove(_) => "remove",
                        EventKind::Access(_) => "access",
                        _ => "other",
                    };
                    for p in paths {
                        yield ResourceEvent::Custom(
                            tag.to_string(),
                            p.to_string_lossy().into_owned().into_bytes(),
                        );
                    }
                }
                Err(e) => {
                    yield ResourceEvent::Custom("error".into(), e.to_string().into_bytes());
                    break;
                }
            }
        }
    };
    Ok(Box::pin(stream))
}

/// Canonicalize helper for tests/diagnostics: the path exactly as the watcher
/// watched it (no FS mutation).
pub fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
