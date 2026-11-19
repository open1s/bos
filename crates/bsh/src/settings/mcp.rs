//! MCP server entries: the typed view over the JSON both settings files hold.
//!
//! The documents keep the entries as JSON because the shape is the MCP client
//! configuration's own: `{name, command, args}` for a child-process server and
//! `{name, url}` for a streamable HTTP one. This module is the one place that
//! knows how to read, validate and write that shape, so the pane and the
//! connection pool cannot disagree about it.

use serde_json::{Map, Value, json};

/// Which document declared an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpScope {
    /// `~/.bos/bsh/settings.toml`, shared by every workspace.
    Global,
    /// `<root>/.bsh/settings/settings.toml`, this workspace only.
    Workspace,
}

impl McpScope {
    /// The scope a row's badge names.
    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Global => "settings.mcpScopeGlobal",
            Self::Workspace => "settings.mcpScopeWorkspace",
        }
    }
}

/// How one server is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpTransport {
    /// A child process spoken to over its stdio pipes.
    Stdio {
        /// The executable to launch.
        command: String,
        /// Its arguments, in order.
        args: Vec<String>,
    },
    /// A streamable HTTP endpoint.
    Http {
        /// The endpoint's URL.
        url: String,
        /// Extra request headers, in the order the pane wrote them.
        ///
        /// This is where a credential lives: `Authorization: Bearer …` or
        /// `X-API-Key: …`. The shape is the `headers` object every MCP client
        /// uses, so an entry pasted from another client keeps its credentials.
        headers: Vec<(String, String)>,
    },
}

/// One configured MCP server as the pane edits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpServer {
    /// The server's name; also the namespace its tools register under.
    pub(crate) name: String,
    /// How it is reached.
    pub(crate) transport: McpTransport,
    /// Whether the pool should connect it.
    pub(crate) enabled: bool,
    /// The document that declared it.
    pub(crate) scope: McpScope,
}

/// Why an edit cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpRefusal {
    /// The name is blank.
    NameEmpty,
    /// The name carries characters a tool namespace cannot.
    NameInvalid,
    /// Another entry in the same document already owns the name.
    NameTaken,
    /// A stdio entry names no executable.
    CommandEmpty,
    /// An HTTP entry names no URL.
    UrlEmpty,
    /// The URL is not an http(s) endpoint.
    UrlScheme,
    /// A header cannot be sent: the name is not a token, or the value spans lines.
    HeaderInvalid,
    /// The entry an edit started from is no longer in its document.
    NotFound,
}

impl McpRefusal {
    /// The locale key the pane draws.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::NameEmpty => "settings.mcpNameEmpty",
            Self::NameInvalid => "settings.mcpNameInvalid",
            Self::NameTaken => "settings.mcpNameTaken",
            Self::CommandEmpty => "settings.mcpNeedsCommand",
            Self::UrlEmpty => "settings.mcpNeedsUrl",
            Self::UrlScheme => "settings.mcpUrlScheme",
            Self::HeaderInvalid => "settings.mcpHeaderInvalid",
            Self::NotFound => "settings.mcpMissing",
        }
    }
}

impl McpServer {
    /// Read one document entry, or none when it is not a server at all.
    ///
    /// An entry with no name cannot be offered to the pool; it is skipped here
    /// rather than guessed at. `enabled` defaults to true, which is the reading
    /// an older entry that predates the field deserves.
    pub(crate) fn parse(value: &Value, scope: McpScope) -> Option<Self> {
        let name = value.get("name")?.as_str()?.trim().to_string();
        if name.is_empty() {
            return None;
        }
        let enabled = value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let transport = match value
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|url| !url.is_empty())
        {
            Some(url) => McpTransport::Http {
                url: url.to_string(),
                headers: read_headers(value.get("headers")),
            },
            None => {
                let command = value
                    .get("command")
                    .and_then(Value::as_str)?
                    .trim()
                    .to_string();
                if command.is_empty() {
                    return None;
                }
                let args = value
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|args| {
                        args.iter()
                            .filter_map(|arg| arg.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                McpTransport::Stdio { command, args }
            }
        };
        Some(Self {
            name,
            transport,
            enabled,
            scope,
        })
    }

    /// Write the entry back in the document's own shape.
    ///
    /// The scope is not serialized: where an entry lives is which document it is
    /// written into, never a field a reader could disagree with the file about.
    pub(crate) fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("name".to_string(), json!(self.name));
        match &self.transport {
            McpTransport::Stdio { command, args } => {
                map.insert("command".to_string(), json!(command));
                if !args.is_empty() {
                    map.insert("args".to_string(), json!(args));
                }
            }
            McpTransport::Http { url, headers } => {
                map.insert("url".to_string(), json!(url));
                if !headers.is_empty() {
                    let mut object = Map::new();
                    for (name, value) in headers {
                        object.insert(name.clone(), Value::String(value.clone()));
                    }
                    map.insert("headers".to_string(), Value::Object(object));
                }
            }
        }
        // Enabled is the default, so only the exception is written.
        if !self.enabled {
            map.insert("enabled".to_string(), json!(false));
        }
        Value::Object(map)
    }

    /// The one-line description of how this server is reached.
    pub(crate) fn transport_summary(&self) -> String {
        match &self.transport {
            McpTransport::Stdio { command, args } if args.is_empty() => {
                format!("stdio · {command}")
            }
            McpTransport::Stdio { command, args } => {
                format!("stdio · {command} {}", args.join(" "))
            }
            McpTransport::Http { url, .. } => format!("http · {url}"),
        }
    }

    /// Identity of the CONNECTION, so an unchanged server is not respawned.
    pub(crate) fn signature(&self) -> String {
        match &self.transport {
            McpTransport::Stdio { command, args } => {
                format!("{}|stdio|{}|{}", self.name, command, args.join(""))
            }
            McpTransport::Http { url, headers } => {
                format!(
                    "{}|http|{}|{}",
                    self.name,
                    url,
                    headers_fingerprint(headers)
                )
            }
        }
    }

    /// Refuse an edit that cannot be saved, against the entries it joins.
    ///
    /// `original` is the name the edit started from, so renaming an entry does
    /// not collide with itself.
    pub(crate) fn validate(
        &self,
        others: &[Self],
        original: Option<&str>,
    ) -> Result<(), McpRefusal> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(McpRefusal::NameEmpty);
        }
        // The name is the tool namespace, so it is the tool-name alphabet rather
        // than free text. Two names that sanitize to the same string would
        // collide in the registry, so a sanitizer is not used: the reader is
        // told to pick a name that already works.
        if !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        {
            return Err(McpRefusal::NameInvalid);
        }
        if let Some(original) = original
            && original == name
        {
            return validate_transport(self);
        }
        if others
            .iter()
            .any(|other| other.name.eq_ignore_ascii_case(name))
        {
            return Err(McpRefusal::NameTaken);
        }
        validate_transport(self)
    }
}

fn validate_transport(server: &McpServer) -> Result<(), McpRefusal> {
    match &server.transport {
        McpTransport::Stdio { command, .. } if command.trim().is_empty() => {
            Err(McpRefusal::CommandEmpty)
        }
        McpTransport::Http { url, headers } => {
            let url = url.trim();
            if url.is_empty() {
                return Err(McpRefusal::UrlEmpty);
            }
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(McpRefusal::UrlScheme);
            }
            if headers
                .iter()
                .any(|(name, value)| !header_name_is_valid(name) || !header_value_is_valid(value))
            {
                return Err(McpRefusal::HeaderInvalid);
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Read the `headers` object an entry carries.
///
/// A non-string value is dropped rather than guessed at, and a blank name is
/// ignored: this is the shape a pasted entry may arrive in.
fn read_headers(value: Option<&Value>) -> Vec<(String, String)> {
    value
        .and_then(Value::as_object)
        .map(|headers| {
            headers
                .iter()
                .filter(|(name, _)| !name.trim().is_empty())
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.trim().to_string(), value.trim().to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A stable fingerprint of the configured headers.
///
/// The signature decides whether a live connection can be reused, so a changed
/// credential must change it. Hashing keeps the credential itself out of any
/// string that reaches a log line.
fn headers_fingerprint(headers: &[(String, String)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    headers.hash(&mut hasher);
    hasher.finish()
}

/// Whether a header name is an RFC 9110 token.
fn header_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    '!' | '#'
                        | '$'
                        | '%'
                        | '&'
                        | '\''
                        | '*'
                        | '+'
                        | '-'
                        | '.'
                        | '^'
                        | '_'
                        | '`'
                        | '|'
                        | '~'
                )
        })
}

/// Whether a header value is a single line of visible text.
fn header_value_is_valid(value: &str) -> bool {
    !value.contains(['\r', '\n'])
}

/// Parse the headers field: one `Name: value` per line.
///
/// A line without a colon, or with a blank name, is skipped: the field is
/// half-typed most of the time, and refusing the whole entry mid-edit would
/// lose the lines already written.
pub(crate) fn parse_headers(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((name.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Render headers back into the form's text, so an edit and a save round-trip.
pub(crate) fn format_headers(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Read every entry one document holds.
pub(crate) fn parse_list(values: &[Value], scope: McpScope) -> Vec<McpServer> {
    values
        .iter()
        .filter_map(|value| McpServer::parse(value, scope))
        .collect()
}

/// The effective list: the global entries in file order, then a workspace entry
/// with the same name replacing its global twin in place.
pub(crate) fn merge(global: &[McpServer], workspace: &[McpServer]) -> Vec<McpServer> {
    let mut merged = global.to_vec();
    for server in workspace {
        match merged
            .iter_mut()
            .find(|existing| existing.name == server.name)
        {
            Some(slot) => *slot = server.clone(),
            None => merged.push(server.clone()),
        }
    }
    merged
}

/// Split the form's argument text the way a shell would.
///
/// The args field is one line of text, so an argument that needs a space has to
/// be quotable: `-m "pkg.module"` is two arguments and the quotes are syntax,
/// never part of the module name. Splitting on whitespace alone kept them, so
/// `python -m "mcp_server_tree_sitter.server"` asked Python for a module
/// literally named `"mcp_server_tree_sitter.server"` and died at once, which the
/// pool could only report as a server that closed its stdout.
///
/// An unmatched quote is not an error: the pane's text is half-typed most of the
/// time, so the quote is dropped and the rest of the line is kept.
pub(crate) fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for character in text.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            started = true;
            continue;
        }
        match character {
            '\\' if !single => {
                escaped = true;
                started = true;
            }
            '\'' if !double => {
                single = !single;
                started = true;
            }
            '"' if !single => {
                double = !double;
                started = true;
            }
            separator if separator.is_whitespace() && !single && !double => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            other => {
                current.push(other);
                started = true;
            }
        }
    }
    if escaped {
        // A trailing backslash is literal text, not a dangling escape.
        current.push('\\');
    }
    if started {
        args.push(current);
    }
    args
}

/// Render args back into the form's text, so an edit and a save round-trip.
pub(crate) fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quote one argument when a [`split_args`] pass would otherwise change it.
fn quote_arg(arg: &str) -> String {
    let needs_quotes = arg.is_empty()
        || arg
            .chars()
            .any(|character| character.is_whitespace() || matches!(character, '"' | '\'' | '\\'));
    if !needs_quotes {
        return arg.to_string();
    }
    let mut quoted = String::from("\"");
    for character in arg.chars() {
        if matches!(character, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_reads_both_transports() {
        let stdio = McpServer::parse(
            &json!({"name": "github", "command": "npx", "args": ["-y", "@mcp/github"]}),
            McpScope::Global,
        )
        .expect("a stdio entry");
        assert_eq!(stdio.name, "github");
        assert_eq!(
            stdio.transport,
            McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@mcp/github".to_string()],
            }
        );
        assert!(stdio.enabled, "enabled is the default");
        assert_eq!(stdio.scope, McpScope::Global);

        let http = McpServer::parse(
            &json!({"name": "docs", "url": "https://mcp.example/v1"}),
            McpScope::Workspace,
        )
        .expect("an http entry");
        assert_eq!(
            http.transport,
            McpTransport::Http {
                url: "https://mcp.example/v1".to_string(),
                headers: Vec::new(),
            }
        );
        assert_eq!(http.scope, McpScope::Workspace);
    }

    #[test]
    fn an_entry_round_trips_and_omits_the_scope() {
        let server = McpServer {
            name: "github".to_string(),
            transport: McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string()],
            },
            enabled: false,
            scope: McpScope::Workspace,
        };
        let value = server.to_value();
        assert_eq!(value["name"], json!("github"));
        assert_eq!(value["command"], json!("npx"));
        assert_eq!(value["args"], json!(["-y"]));
        assert_eq!(value["enabled"], json!(false));
        assert!(
            value.get("scope").is_none(),
            "scope is the file, not a field"
        );
        assert_eq!(McpServer::parse(&value, McpScope::Workspace), Some(server));
    }

    #[test]
    fn a_nameless_entry_is_not_a_server() {
        assert!(McpServer::parse(&json!({"command": "npx"}), McpScope::Global).is_none());
        assert!(McpServer::parse(&json!({"name": "  "}), McpScope::Global).is_none());
        assert!(McpServer::parse(&json!({"name": "x"}), McpScope::Global).is_none());
    }

    #[test]
    fn a_name_must_be_a_tool_namespace() {
        let base = McpServer {
            name: "my server".to_string(),
            transport: McpTransport::Stdio {
                command: "npx".to_string(),
                args: Vec::new(),
            },
            enabled: true,
            scope: McpScope::Global,
        };
        assert_eq!(base.validate(&[], None), Err(McpRefusal::NameInvalid));
        let mut ok = base.clone();
        ok.name = "my-server_2".to_string();
        assert_eq!(ok.validate(&[], None), Ok(()));
    }

    #[test]
    fn a_duplicate_name_is_refused_unless_it_is_the_entry_being_renamed() {
        let existing = McpServer {
            name: "github".to_string(),
            transport: McpTransport::Http {
                url: "https://a.example".to_string(),
                headers: Vec::new(),
            },
            enabled: true,
            scope: McpScope::Global,
        };
        let mut candidate = existing.clone();
        candidate.transport = McpTransport::Http {
            url: "https://b.example".to_string(),
            headers: Vec::new(),
        };
        assert_eq!(
            candidate.validate(std::slice::from_ref(&existing), None),
            Err(McpRefusal::NameTaken)
        );
        assert_eq!(
            candidate.validate(std::slice::from_ref(&existing), Some("github")),
            Ok(()),
            "editing the entry keeps its own name"
        );
    }

    #[test]
    fn a_missing_transport_field_is_refused() {
        let no_command = McpServer {
            name: "a".to_string(),
            transport: McpTransport::Stdio {
                command: "  ".to_string(),
                args: Vec::new(),
            },
            enabled: true,
            scope: McpScope::Global,
        };
        assert_eq!(
            no_command.validate(&[], None),
            Err(McpRefusal::CommandEmpty)
        );
        let no_url = McpServer {
            name: "a".to_string(),
            transport: McpTransport::Http {
                url: "  ".to_string(),
                headers: Vec::new(),
            },
            enabled: true,
            scope: McpScope::Global,
        };
        assert_eq!(no_url.validate(&[], None), Err(McpRefusal::UrlEmpty));
        let bad_url = McpServer {
            name: "a".to_string(),
            transport: McpTransport::Http {
                url: "mcp.example".to_string(),
                headers: Vec::new(),
            },
            enabled: true,
            scope: McpScope::Global,
        };
        assert_eq!(bad_url.validate(&[], None), Err(McpRefusal::UrlScheme));
    }

    #[test]
    fn the_workspace_entry_wins_by_name_and_keeps_the_order() {
        let global = vec![
            McpServer {
                name: "one".to_string(),
                transport: McpTransport::Http {
                    url: "https://global.example/one".to_string(),
                    headers: Vec::new(),
                },
                enabled: true,
                scope: McpScope::Global,
            },
            McpServer {
                name: "two".to_string(),
                transport: McpTransport::Http {
                    url: "https://global.example/two".to_string(),
                    headers: Vec::new(),
                },
                enabled: true,
                scope: McpScope::Global,
            },
        ];
        let workspace = vec![
            McpServer {
                name: "two".to_string(),
                transport: McpTransport::Http {
                    url: "https://local.example/two".to_string(),
                    headers: Vec::new(),
                },
                enabled: false,
                scope: McpScope::Workspace,
            },
            McpServer {
                name: "three".to_string(),
                transport: McpTransport::Http {
                    url: "https://local.example/three".to_string(),
                    headers: Vec::new(),
                },
                enabled: true,
                scope: McpScope::Workspace,
            },
        ];
        let merged = merge(&global, &workspace);
        let names: Vec<&str> = merged.iter().map(|server| server.name.as_str()).collect();
        assert_eq!(names, ["one", "two", "three"]);
        assert_eq!(merged[1].scope, McpScope::Workspace);
        assert!(!merged[1].enabled, "the override carries its own state");
    }

    #[test]
    fn the_signature_tracks_the_connection_not_the_queue_position() {
        let http = McpServer {
            name: "docs".to_string(),
            transport: McpTransport::Http {
                url: "https://a.example".to_string(),
                headers: Vec::new(),
            },
            enabled: true,
            scope: McpScope::Global,
        };
        let mut moved = http.clone();
        moved.scope = McpScope::Workspace;
        moved.enabled = false;
        assert_eq!(
            moved.signature(),
            http.signature(),
            "moving or disabling an entry does not change what it connects to"
        );
        let mut reconnected = http.clone();
        reconnected.transport = McpTransport::Http {
            url: "https://b.example".to_string(),
            headers: Vec::new(),
        };
        assert_ne!(reconnected.signature(), http.signature());
    }

    #[test]
    fn argument_text_splits_the_way_a_shell_does() {
        assert_eq!(
            split_args(r#"-m "mcp_server_tree_sitter.server" --root "/a b""#),
            vec![
                "-m".to_string(),
                "mcp_server_tree_sitter.server".to_string(),
                "--root".to_string(),
                "/a b".to_string(),
            ]
        );
        assert_eq!(split_args(" -y   ''  x "), vec!["-y", "", "x"]);
        assert_eq!(split_args(r"a\ b"), vec!["a b"]);
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(split_args(r"trailing\"), vec!["trailing\\"]);
        assert_eq!(split_args("'single quoted'"), vec!["single quoted"]);
        assert_eq!(split_args(r#"x="a b" y"#), vec!["x=a b", "y"]);
    }

    #[test]
    fn args_round_trip_through_the_form_text() {
        let args = vec![
            "-m".to_string(),
            "mcp_server_tree_sitter.server".to_string(),
            "/a b".to_string(),
            "quote\"inside".to_string(),
            String::new(),
            "back\\slash".to_string(),
        ];
        assert_eq!(split_args(&join_args(&args)), args);
    }

    #[test]
    fn an_http_entry_keeps_its_headers() {
        let server = McpServer::parse(
            &json!({
                "name": "broker",
                "url": "http://127.0.0.1:8080",
                "headers": {"Authorization": "Bearer secret", "X-API-Key": "k"}
            }),
            McpScope::Global,
        )
        .expect("an http entry");
        let McpTransport::Http { headers, .. } = &server.transport else {
            panic!("an http entry is an HTTP transport");
        };
        assert_eq!(headers.len(), 2, "{headers:?}");
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "Authorization" && value == "Bearer secret"),
            "{headers:?}"
        );
        let written = server.to_value();
        assert_eq!(written["headers"]["Authorization"], json!("Bearer secret"));
    }

    #[test]
    fn a_header_that_cannot_be_sent_is_refused() {
        let server = McpServer {
            name: "broker".to_string(),
            transport: McpTransport::Http {
                url: "http://127.0.0.1:8080".to_string(),
                headers: vec![("Bad Name".to_string(), "x".to_string())],
            },
            enabled: true,
            scope: McpScope::Global,
        };
        assert_eq!(server.validate(&[], None), Err(McpRefusal::HeaderInvalid));
        let injected = McpServer {
            transport: McpTransport::Http {
                url: "http://127.0.0.1:8080".to_string(),
                headers: vec![("X-Key".to_string(), "a\r\nInjected: 1".to_string())],
            },
            ..server
        };
        assert_eq!(injected.validate(&[], None), Err(McpRefusal::HeaderInvalid));
    }

    #[test]
    fn a_changed_credential_reconnects() {
        let server = McpServer {
            name: "broker".to_string(),
            transport: McpTransport::Http {
                url: "http://127.0.0.1:8080".to_string(),
                headers: vec![("Authorization".to_string(), "Bearer one".to_string())],
            },
            enabled: true,
            scope: McpScope::Global,
        };
        let mut changed = server.clone();
        if let McpTransport::Http { headers, .. } = &mut changed.transport {
            headers[0].1 = "Bearer two".to_string();
        }
        assert_ne!(server.signature(), changed.signature());
        assert_eq!(server.signature(), server.clone().signature());
    }

    #[test]
    fn headers_round_trip_through_the_form_text() {
        let headers = vec![
            ("Authorization".to_string(), "Bearer a b".to_string()),
            ("X-API-Key".to_string(), "k:v".to_string()),
        ];
        assert_eq!(parse_headers(&format_headers(&headers)), headers);
        assert_eq!(
            parse_headers("Authorization:  x  \n\nbare line\n: empty"),
            vec![("Authorization".to_string(), "x".to_string())]
        );
    }
}
