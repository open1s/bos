//! What an MCP server publishes besides tools: its resources and its prompts.
//!
//! `McpClient` has spoken `resources/list`, `resources/read` and `prompts/list`
//! for as long as it has spoken `tools/list` — and nothing in this GUI ever
//! called them. A server whose whole point was a resource (a schema, a runbook,
//! a database catalog) could be connected, handshaked, listed in the pane, and
//! still not readable from the app. This module is the pane's side of that
//! seam: what one server offers, read on demand.
//!
//! Three rules, and each one is here rather than in the caller:
//!
//! * **A capability the server never declared is not asked about.** `false` is
//!   the protocol's way of saying "I do not do this", and calling
//!   `resources/list` on such a server is a guaranteed round trip to an error
//!   path. The pane says "not offered" instead of showing a failure.
//! * **One server's failure cannot hide another's rows.** A `resources/list`
//!   that fails is that server's line; the rest of the catalog still draws.
//! * **Everything a server says is bounded** — rows, description length and the
//!   characters of one resource body — because this text comes from a process
//!   the user configured but did not write, and it is drawn in a dialog.
//!
//! The text of a resource is deliberately *not* injected anywhere: reading one
//! is a gesture (the pane, or the user pasting it), never a side effect of a
//! server being connected.

use serde::Serialize;

use agent::mcp::{McpPrompt, McpResource, ReadResourceResult, ServerCapabilities};

/// Most rows (resources plus prompts) drawn per server.
pub(crate) const MAX_ROWS: usize = 200;

/// Most characters of one resource body handed to the pane.
pub(crate) const MAX_TEXT_CHARS: usize = 20_000;

/// Most characters of a description or name drawn per row.
pub(crate) const MAX_DESC_CHARS: usize = 240;

/// Most characters in a URI the pane will send to a server.
pub(crate) const MAX_URI_CHARS: usize = 2_000;

/// One resource, as the pane draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ResourceRow {
    /// Resource URI, the argument `resources/read` takes.
    pub(crate) uri: String,
    /// Human-readable name.
    pub(crate) name: String,
    /// Bounded description.
    pub(crate) description: String,
    /// Optional MIME type the server advertised.
    pub(crate) mime_type: Option<String>,
    /// Whether the name or description was cut to fit.
    pub(crate) clipped: bool,
}

/// One prompt, as the pane draws it. A prompt is listed for reference: the
/// client has no `prompts/get`, so the pane cannot fetch one yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PromptRow {
    /// Prompt name.
    pub(crate) name: String,
    /// Bounded description.
    pub(crate) description: String,
    /// How many arguments the prompt takes.
    pub(crate) arguments: usize,
    /// How many of those are required.
    pub(crate) required: usize,
    /// Whether the name or description was cut to fit.
    pub(crate) clipped: bool,
}

/// One server's slice of the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ServerView {
    /// Server name (namespace) as configured.
    pub(crate) server: String,
    /// Whether this pass had a live client for it.
    pub(crate) connected: bool,
    /// Whether the server declared the `resources` capability.
    pub(crate) resources_supported: bool,
    /// Whether the server declared the `prompts` capability.
    pub(crate) prompts_supported: bool,
    /// Resources drawn (bounded by [`MAX_ROWS`]).
    pub(crate) resources: Vec<ResourceRow>,
    /// Prompts drawn (bounded by [`MAX_ROWS`]).
    pub(crate) prompts: Vec<PromptRow>,
    /// How many resources the server listed, before bounding.
    pub(crate) resources_total: usize,
    /// How many prompts the server listed, before bounding.
    pub(crate) prompts_total: usize,
    /// Failure from `resources/list`, for this server alone.
    pub(crate) error: Option<String>,
}

/// The whole catalog, as one command answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Catalog {
    /// Caller's clock, so a pane can say when it last looked.
    pub(crate) now_ms: u64,
    /// One entry per configured server, in settings order.
    pub(crate) servers: Vec<ServerView>,
    /// The bounds this build applied, so the pane can state them.
    pub(crate) max_rows: usize,
    /// Bound on one resource body.
    pub(crate) max_text_chars: usize,
    /// Bound on a row's name or description.
    pub(crate) max_desc_chars: usize,
}

/// One resource's text, as the pane draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReadView {
    /// Caller's clock.
    pub(crate) now_ms: u64,
    /// Server the resource came from.
    pub(crate) server: String,
    /// URI that was read.
    pub(crate) uri: String,
    /// MIME type of the first part that carried one.
    pub(crate) mime_type: Option<String>,
    /// The text, joined across parts and bounded by [`MAX_TEXT_CHARS`].
    pub(crate) text: String,
    /// Characters actually handed over.
    pub(crate) chars: usize,
    /// Characters the server sent, before bounding.
    pub(crate) source_chars: usize,
    /// Whether the body was cut to fit.
    pub(crate) truncated: bool,
    /// How many content parts the server returned.
    pub(crate) parts: usize,
    /// Whether the server returned no text at all (a binary resource).
    pub(crate) empty: bool,
}

/// Whether a server's advertised capability means "I do this".
///
/// The protocol spells the negative either as `false` or by omitting the field;
/// both deserialize to `false` here. Anything else — an object of sub-options,
/// or a bare `true` — is a yes.
pub(crate) fn supported(capability: &serde_json::Value) -> bool {
    !matches!(
        capability,
        serde_json::Value::Bool(false) | serde_json::Value::Null
    )
}

/// Cut `text` to `max` **characters**, reporting what the source held.
///
/// Characters, not bytes: a body of 2000 emoji is 2000 characters and 8000
/// bytes, and a byte-bound cut would both under-report the source and slice a
/// character in half.
pub(crate) fn clip(text: &str, max: usize) -> (String, usize, bool) {
    let source = text.chars().count();
    if source <= max {
        return (text.to_string(), source, false);
    }
    (text.chars().take(max).collect(), source, true)
}

/// One resource row, with its name and description bounded.
pub(crate) fn resource_row(resource: &McpResource) -> ResourceRow {
    let (name, name_source, name_cut) = clip(&resource.name, MAX_DESC_CHARS);
    let (description, desc_source, desc_cut) = clip(&resource.description, MAX_DESC_CHARS);
    ResourceRow {
        uri: resource.uri.clone(),
        name,
        description,
        mime_type: resource.mime_type.clone(),
        clipped: name_cut
            || desc_cut
            || name_source > MAX_DESC_CHARS
            || desc_source > MAX_DESC_CHARS,
    }
}

/// One prompt row, with its name and description bounded.
pub(crate) fn prompt_row(prompt: &McpPrompt) -> PromptRow {
    let (name, _, name_cut) = clip(&prompt.name, MAX_DESC_CHARS);
    let (description, _, desc_cut) = clip(&prompt.description, MAX_DESC_CHARS);
    let arguments = prompt.arguments.as_ref().map(Vec::len).unwrap_or(0);
    let required = prompt
        .arguments
        .as_ref()
        .map(|args| args.iter().filter(|a| a.required == Some(true)).count())
        .unwrap_or(0);
    PromptRow {
        name,
        description,
        arguments,
        required,
        clipped: name_cut || desc_cut,
    }
}

/// Assemble one server's view from what its client answered.
///
/// `resources` is the client's result: `Err` is this server's failure and is
/// kept as a line rather than raised, because the catalog is a summary of many
/// servers. Unsupported capabilities are emptied here as well as by the caller,
/// so a row list cannot be drawn under a "not offered" heading.
pub(crate) fn server_view(
    server: &str,
    connected: bool,
    capabilities: Option<&ServerCapabilities>,
    resources: Result<Vec<McpResource>, String>,
    prompts: Vec<McpPrompt>,
) -> ServerView {
    let resources_supported = capabilities
        .map(|c| supported(&c.resources))
        .unwrap_or(false);
    let prompts_supported = capabilities.map(|c| supported(&c.prompts)).unwrap_or(false);

    let (rows, error) = match resources {
        Ok(listed) => (listed, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    // The total is part of the same promise as the rows: a capability the
    // server said it does not have reports zero, not the count of rows the
    // caller happened to hand in.
    let resources_total = if resources_supported { rows.len() } else { 0 };
    let rows: Vec<ResourceRow> = if resources_supported {
        rows.iter().take(MAX_ROWS).map(resource_row).collect()
    } else {
        Vec::new()
    };

    let prompts_total = if prompts_supported { prompts.len() } else { 0 };
    let prompts: Vec<PromptRow> = if prompts_supported {
        prompts.iter().take(MAX_ROWS).map(prompt_row).collect()
    } else {
        Vec::new()
    };

    ServerView {
        server: server.to_string(),
        connected,
        resources_supported,
        prompts_supported,
        resources: rows,
        prompts,
        resources_total,
        prompts_total,
        error,
    }
}

/// Assemble one resource's text from what the client answered.
pub(crate) fn read_view(
    server: &str,
    uri: &str,
    result: &ReadResourceResult,
    now_ms: u64,
) -> ReadView {
    let text: String = result
        .contents
        .iter()
        .filter_map(|part| part.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mime_type = result
        .contents
        .iter()
        .find_map(|part| part.mime_type.clone());
    let (text, source_chars, truncated) = clip(&text, MAX_TEXT_CHARS);
    ReadView {
        now_ms,
        server: server.to_string(),
        uri: uri.to_string(),
        mime_type,
        chars: text.chars().count(),
        text,
        source_chars,
        truncated,
        parts: result.contents.len(),
        empty: source_chars == 0,
    }
}

/// Wrap the per-server views with the bounds this build applied.
pub(crate) fn catalog(now_ms: u64, servers: Vec<ServerView>) -> Catalog {
    Catalog {
        now_ms,
        servers,
        max_rows: MAX_ROWS,
        max_text_chars: MAX_TEXT_CHARS,
        max_desc_chars: MAX_DESC_CHARS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::mcp::{McpPromptArgument, ResourceContents};

    fn resource(uri: &str, name: &str) -> McpResource {
        McpResource {
            uri: uri.to_string(),
            name: name.to_string(),
            description: format!("what {name} is"),
            mime_type: Some("text/plain".to_string()),
        }
    }

    fn caps(resources: bool, prompts: bool) -> ServerCapabilities {
        ServerCapabilities {
            tools: serde_json::json!({}),
            resources: if resources {
                serde_json::json!({ "subscribe": false })
            } else {
                serde_json::Value::Bool(false)
            },
            prompts: if prompts {
                serde_json::json!({})
            } else {
                serde_json::Value::Bool(false)
            },
        }
    }

    #[test]
    fn an_undeclared_capability_is_false_and_anything_else_is_true() {
        assert!(!supported(&serde_json::Value::Bool(false)));
        assert!(!supported(&serde_json::Value::Null));
        assert!(supported(&serde_json::json!({})));
        assert!(supported(&serde_json::Value::Bool(true)));
        assert!(supported(&serde_json::json!({ "subscribe": true })));
    }

    #[test]
    fn clipping_counts_characters_not_bytes() {
        let body = "é".repeat(10);
        assert_eq!(body.len(), 20, "two bytes each");
        let (out, source, truncated) = clip(&body, 4);
        assert_eq!(out, "éééé");
        assert_eq!(out.chars().count(), 4);
        assert_eq!(source, 10);
        assert!(truncated);

        let (whole, source, truncated) = clip("short", 40);
        assert_eq!(whole, "short");
        assert_eq!(source, 5);
        assert!(!truncated);
    }

    #[test]
    fn a_capability_the_server_never_declared_shows_no_rows() {
        let listed: Vec<McpResource> = (0..3)
            .map(|i| resource(&format!("file:///{i}"), "doc"))
            .collect();
        let view = server_view(
            "srv",
            true,
            Some(&caps(false, false)),
            Ok(listed),
            vec![McpPrompt {
                name: "summarize".to_string(),
                description: String::new(),
                arguments: None,
            }],
        );
        assert!(!view.resources_supported && !view.prompts_supported);
        assert!(view.resources.is_empty(), "{:?}", view.resources);
        assert!(view.prompts.is_empty(), "{:?}", view.prompts);
        assert_eq!(view.resources_total, 0);
        assert_eq!(view.prompts_total, 0);
        assert!(view.error.is_none());
    }

    #[test]
    fn one_servers_failure_does_not_hide_anothers_rows() {
        let views = [
            server_view(
                "broken",
                true,
                Some(&caps(true, true)),
                Err("boom".to_string()),
                Vec::new(),
            ),
            server_view(
                "fine",
                true,
                Some(&caps(true, false)),
                Ok(vec![resource("file:///a", "a")]),
                Vec::new(),
            ),
        ];
        assert_eq!(views[0].error.as_deref(), Some("boom"));
        assert!(views[0].resources.is_empty());
        assert_eq!(views[1].resources.len(), 1);
        assert_eq!(views[1].resources[0].uri, "file:///a");
        assert!(views[1].error.is_none());
    }

    #[test]
    fn more_rows_than_the_bound_are_cut_and_the_total_is_kept() {
        let many: Vec<McpResource> = (0..MAX_ROWS + 7)
            .map(|i| resource(&format!("file:///{i}"), "doc"))
            .collect();
        let view = server_view("srv", true, Some(&caps(true, false)), Ok(many), vec![]);
        assert_eq!(view.resources.len(), MAX_ROWS);
        assert_eq!(view.resources_total, MAX_ROWS + 7);

        let long = McpResource {
            uri: "file:///long".to_string(),
            name: "n".repeat(MAX_DESC_CHARS + 40),
            description: "d".repeat(MAX_DESC_CHARS + 40),
            mime_type: None,
        };
        let row = resource_row(&long);
        assert_eq!(row.name.chars().count(), MAX_DESC_CHARS);
        assert_eq!(row.description.chars().count(), MAX_DESC_CHARS);
        assert!(row.clipped);
    }

    #[test]
    fn a_prompt_row_counts_its_arguments_and_the_required_ones() {
        let prompt = McpPrompt {
            name: "review".to_string(),
            description: "Review a diff".to_string(),
            arguments: Some(vec![
                McpPromptArgument {
                    name: "diff".to_string(),
                    description: "the diff".to_string(),
                    required: Some(true),
                },
                McpPromptArgument {
                    name: "style".to_string(),
                    description: "house style".to_string(),
                    required: None,
                },
            ]),
        };
        let row = prompt_row(&prompt);
        assert_eq!(row.arguments, 2);
        assert_eq!(row.required, 1);
        assert!(!row.clipped);

        let bare = prompt_row(&McpPrompt {
            name: "bare".to_string(),
            description: String::new(),
            arguments: None,
        });
        assert_eq!((bare.arguments, bare.required), (0, 0));
    }

    #[test]
    fn a_read_joins_its_parts_and_reports_the_bytes_it_did_not_send() {
        let result = ReadResourceResult {
            contents: vec![
                ResourceContents {
                    uri: "file:///a".to_string(),
                    mime_type: Some("text/markdown".to_string()),
                    text: Some("first".to_string()),
                },
                ResourceContents {
                    uri: "file:///a".to_string(),
                    mime_type: None,
                    text: Some("second".to_string()),
                },
            ],
        };
        let read = read_view("srv", "file:///a", &result, 7);
        assert_eq!(read.text, "first\n\nsecond");
        assert_eq!(read.chars, 13);
        assert_eq!(read.source_chars, 13);
        assert!(!read.truncated);
        assert_eq!(read.parts, 2);
        assert!(!read.empty);
        assert_eq!(read.mime_type.as_deref(), Some("text/markdown"));
        assert_eq!(read.now_ms, 7);

        let huge = ReadResourceResult {
            contents: vec![ResourceContents {
                uri: "file:///big".to_string(),
                mime_type: None,
                text: Some("x".repeat(MAX_TEXT_CHARS + 500)),
            }],
        };
        let read = read_view("srv", "file:///big", &huge, 0);
        assert!(read.truncated);
        assert_eq!(read.chars, MAX_TEXT_CHARS);
        assert_eq!(read.source_chars, MAX_TEXT_CHARS + 500);
        assert!(read.text.ends_with('x'));
    }

    #[test]
    fn a_resource_with_no_text_says_so_instead_of_reading_empty() {
        let result = ReadResourceResult {
            contents: vec![ResourceContents {
                uri: "file:///blob.png".to_string(),
                mime_type: Some("image/png".to_string()),
                text: None,
            }],
        };
        let read = read_view("srv", "file:///blob.png", &result, 0);
        assert!(read.empty, "{read:?}");
        assert_eq!(read.text, "");
        assert_eq!(read.chars, 0);
        assert_eq!(read.parts, 1);
        assert_eq!(read.mime_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn the_catalog_carries_the_bounds_it_applied() {
        let report = catalog(
            11,
            vec![server_view("srv", false, None, Ok(vec![]), vec![])],
        );
        assert_eq!(report.now_ms, 11);
        assert_eq!(report.max_rows, MAX_ROWS);
        assert_eq!(report.max_text_chars, MAX_TEXT_CHARS);
        assert_eq!(report.max_desc_chars, MAX_DESC_CHARS);
        assert_eq!(report.servers.len(), 1);
        assert!(!report.servers[0].connected);
        assert!(!report.servers[0].resources_supported);
    }
}
