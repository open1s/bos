//! Agent-authored surfaces.
//!
//! A tool call stores a validated schema 1.1 document here; the panel list
//! draws it as a read-only surface under an agent badge. The store is the same
//! one the surface host validates and owns, so an agent cannot ship a document
//! the framework would refuse, and closing a surface disposes its panel.
//!
//! Actions inside an agent surface are not wired to the capability pipeline
//! yet: the panel is presented with no tools, so a control reports that the
//! action is unavailable rather than pretending to run. That is deliberate -
//! a half-authorised action is worse than a disabled one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ferris_sdk::host_runtime::{CallContext, Runtime, ToolDescriptor};
use ferris_sdk::surface::frame::apply_patch;
use ferris_sdk::surface::{PatchOp, Revision, SurfaceDocument, SurfaceLimits};
use ferris_sdk::types::{ExtensionError, RuntimeKind};
use ferris_sdk::ui::PanelLocation;
use serde_json::Value;

/// How many agent surfaces may exist at once.
pub(crate) const MAX_AGENT_SURFACES: usize = 3;

/// One presented surface.
pub(crate) struct AgentSurface {
    /// Stable id, also the panel contribution id.
    pub(crate) id: String,
    /// The label the agent supplied.
    pub(crate) title: String,
    /// Where the host draws it.
    pub(crate) location: PanelLocation,
    /// The current document.
    pub(crate) document: SurfaceDocument,
    /// The revision, advanced by patches.
    pub(crate) revision: Revision,
    /// The runtime that serves the document to the panel.
    pub(crate) runtime: Arc<AgentSurfaceRuntime>,
}

/// The presented surfaces.
fn store() -> &'static Mutex<Vec<AgentSurface>> {
    static STORE: OnceLock<Mutex<Vec<AgentSurface>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Vec::new()))
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Present a document. Validates against the agent limits first.
pub(crate) fn present(
    title: &str,
    location: PanelLocation,
    document: SurfaceDocument,
) -> Result<String, String> {
    let limits = SurfaceLimits::agent();
    let report = document.validate_with(&limits);
    if !report.is_valid() {
        return Err(report
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "));
    }
    let mut store = store()
        .lock()
        .map_err(|_| "the surface store is poisoned".to_string())?;
    if store.len() >= MAX_AGENT_SURFACES {
        return Err(format!(
            "at most {MAX_AGENT_SURFACES} agent surfaces may be open; close one first"
        ));
    }
    let id = format!("surface-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let runtime = Arc::new(AgentSurfaceRuntime {
        document: Mutex::new(document.clone()),
    });
    store.push(AgentSurface {
        id: id.clone(),
        title: title.trim().to_string(),
        location,
        document,
        revision: Revision::initial(),
        runtime,
    });
    Ok(id)
}

/// Apply a base-relative patch to a presented surface.
pub(crate) fn patch(id: &str, base: Revision, ops: Vec<PatchOp>) -> Result<Revision, String> {
    let mut store = store()
        .lock()
        .map_err(|_| "the surface store is poisoned".to_string())?;
    let entry = store
        .iter_mut()
        .find(|surface| surface.id == id)
        .ok_or_else(|| format!("no agent surface {id}"))?;
    if entry.revision != base {
        return Err(format!(
            "stale patch base {}; the surface is at {}",
            base.value(),
            entry.revision.value()
        ));
    }
    let next = apply_patch(&entry.document, &ops, &SurfaceLimits::agent())
        .map_err(|error| error.to_string())?;
    entry.document = next.clone();
    entry.revision = entry.revision.next();
    entry.runtime.set(next);
    Ok(entry.revision)
}

/// Close a presented surface.
pub(crate) fn close(id: &str) -> bool {
    let Ok(mut store) = store().lock() else {
        return false;
    };
    let before = store.len();
    store.retain(|surface| surface.id != id);
    store.len() != before
}

/// A snapshot of the presented surfaces, for the panel list.
pub(crate) fn views() -> Vec<AgentSurfaceView> {
    let Ok(store) = store().lock() else {
        return Vec::new();
    };
    store
        .iter()
        .map(|surface| AgentSurfaceView {
            id: surface.id.clone(),
            title: surface.title.clone(),
            location: surface.location,
            document: surface.document.clone(),
            runtime: Arc::clone(&surface.runtime) as Arc<dyn Runtime>,
        })
        .collect()
}

/// One presented surface as the panel list needs it.
#[derive(Clone)]
pub(crate) struct AgentSurfaceView {
    /// Stable id.
    pub(crate) id: String,
    /// Agent-supplied title.
    pub(crate) title: String,
    /// Where to draw it.
    pub(crate) location: PanelLocation,
    /// The current document.
    pub(crate) document: SurfaceDocument,
    /// The runtime that serves it.
    pub(crate) runtime: Arc<dyn Runtime>,
}

/// The component catalogue an agent is offered.
pub(crate) fn catalogue() -> Value {
    let limits = SurfaceLimits::agent();
    let slots: Vec<&str> = crate::extension::surface::HostSlot::names()
        .into_iter()
        .filter(|name| crate::extension::surface::HostSlot::agent_slot(name).is_some())
        .collect();
    serde_json::json!({
        "schema_version": "1.1",
        "limits": {
            "max_nodes": limits.max_nodes,
            "max_bytes": limits.max_bytes,
        },
        "node_types": [
            "container", "row", "column", "text", "heading", "markdown", "button",
            "input", "select", "checkbox", "list", "table", "section", "tree",
            "tabs", "progress", "code", "image", "notification", "divider",
            "spacer", "slot"
        ],
        "required": {
            "interactive": "every button, input, select and checkbox carries a unique id",
            "bindings": "controls name a key in the document binding table",
        },
        "slots": slots,
    })
}

/// The runtime that serves one agent surface's document.
pub(crate) struct AgentSurfaceRuntime {
    document: Mutex<SurfaceDocument>,
}

impl AgentSurfaceRuntime {
    fn set(&self, document: SurfaceDocument) {
        if let Ok(mut current) = self.document.lock() {
            *current = document;
        }
    }
}

#[async_trait::async_trait]
impl Runtime for AgentSurfaceRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Builtin
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        Ok(Vec::new())
    }

    async fn call_tool(
        &self,
        name: &str,
        _arguments: Value,
        _context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        Err(ExtensionError::NotFound {
            id: name.to_string(),
        })
    }

    async fn panel(&self, _id: &str) -> Result<Value, ExtensionError> {
        let document = self.document.lock().map_err(|_| ExtensionError::Protocol {
            extension: "bos.agent".to_string(),
            message: "the agent surface is poisoned".to_string(),
        })?;
        Ok(serde_json::to_value(document.to_ui_document()).unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_sdk::surface::frame::PatchOp;
    use ferris_sdk::surface::{SurfaceNode, SURFACE_SCHEMA_VERSION};

    static LOCK: Mutex<()> = Mutex::new(());

    fn text(id: Option<&str>, value: &str) -> SurfaceNode {
        SurfaceNode::Text {
            id: id.map(ToString::to_string),
            text: value.to_string(),
            tone: ferris_sdk::ui::Tone::Neutral,
        }
    }

    fn document(value: &str) -> SurfaceDocument {
        SurfaceDocument {
            schema_version: SURFACE_SCHEMA_VERSION.to_string(),
            bindings: std::collections::BTreeMap::new(),
            hidden_actions: Vec::new(),
            root: SurfaceNode::Column {
                id: None,
                children: vec![text(Some("body"), value)],
            },
        }
    }

    struct SurfaceColumn;
    impl SurfaceColumn {
        fn column(children: Vec<SurfaceNode>) -> SurfaceNode {
            SurfaceNode::Column { id: None, children }
        }
    }

    #[test]
    fn present_patch_and_close() {
        let _guard = LOCK.lock().expect("test lock");
        let id = present("Status", PanelLocation::Right, document("one")).expect("present");
        assert!(views().iter().any(|view| view.id == id));
        let revision = patch(
            &id,
            Revision::new(1),
            vec![PatchOp::SetText {
                id: "body".to_string(),
                text: "two".to_string(),
            }],
        )
        .expect("patch");
        assert_eq!(revision.value(), 2);
        assert!(patch(&id, Revision::new(1), vec![]).is_err());
        assert!(close(&id));
        assert!(!views().iter().any(|view| view.id == id));
    }

    #[test]
    fn an_invalid_document_is_refused_before_it_is_stored() {
        let _guard = LOCK.lock().expect("test lock");
        let invalid = SurfaceDocument {
            schema_version: SURFACE_SCHEMA_VERSION.to_string(),
            bindings: std::collections::BTreeMap::new(),
            hidden_actions: Vec::new(),
            root: SurfaceNode::Button {
                id: String::new(),
                label: "Go".to_string(),
                action: "go".to_string(),
                style: ferris_sdk::ui::ButtonStyle::Primary,
            },
        };
        assert!(present("Bad", PanelLocation::Right, invalid).is_err());
    }

    #[tokio::test]
    async fn the_runtime_serves_the_current_document() {
        // The lock covers the shared-store mutations only; holding a sync
        // MutexGuard across the await would serialise the runtime on a lock
        // the async executor cannot see.
        let (id, view) = {
            let _guard = LOCK.lock().expect("test lock");
            let id = present("Served", PanelLocation::Right, document("live")).expect("present");
            let view = views()
                .into_iter()
                .find(|view| view.id == id)
                .expect("view");
            patch(
                &id,
                Revision::new(1),
                vec![PatchOp::SetText {
                    id: "body".to_string(),
                    text: "patched".to_string(),
                }],
            )
            .expect("patch");
            (id, view)
        };
        let value = view.runtime.panel("panel").await.expect("panel");
        let json = serde_json::to_string(&value).expect("json");
        assert!(json.contains("patched"), "{json}");
        let _guard = LOCK.lock().expect("test lock");
        assert!(close(&id));
    }
}
