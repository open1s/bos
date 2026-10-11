//! Read-only web tools: fetch a URL's readable text, and search the web.
//!
//! Both are plain async tools registered on the chat agent, so they work with
//! any vendor — not just the ones whose API ships a hosted search tool. They
//! are read-only (no approval gate, the same standing as `read_file`), and the
//! agent reaches the network the way the user's browser would.
//!
//! Search answers from DuckDuckGo's HTML endpoint by default, which needs no
//! key; a Tavily-style endpoint and key in the settings switch the transport
//! to a JSON API without touching the tool's schema.

use std::sync::Arc;

use agent::tools::{AsyncFunctionTool, AsyncTool};

/// Characters of page text one fetch keeps.
const MAX_FETCH_CHARS: usize = 20_000;
/// Bytes of body one fetch reads before decoding (the cap applies to text).
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Seconds before a fetch or a search gives up.
const TIMEOUT_SECS: u64 = 15;
/// Results one search returns.
const MAX_RESULTS: usize = 8;

/// Build the `web_fetch` tool.
///
/// Args: `url` (required). Returns `{url, status, content_type, title, text}`.
/// HTML bodies are reduced to text (scripts, styles and tags stripped, the
/// few entities decoded); anything else is passed through as text.
pub(crate) fn web_fetch_tool() -> Arc<dyn AsyncTool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "url": { "type": "string", "description": "http(s) URL to fetch" },
            "method": {
                "type": "string",
                "description": "HTTP method (default GET)",
                "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]
            },
            "body": {
                "type": "string",
                "description": "request body for POST/PUT/PATCH; a body without a \
                     Content-Type header is sent as application/json"
            },
            "headers": {
                "type": "object",
                "description": "extra request headers, name -> value",
                "additionalProperties": { "type": "string" }
            }
        },
        "required": ["url"],
        "additionalProperties": false
    });
    Arc::new(AsyncFunctionTool::new(
        "web_fetch",
        "Fetch a URL and return its readable text (HTML reduced to text; title, \
         status and content type included). Any HTTP method works: pass `method` \
         (GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS), an optional `body` and extra \
         `headers`, e.g. to call a JSON API. Response bodies stay text.",
        schema,
        |args| {
            let url = args
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let method = args
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_string();
            let body = args
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let headers = args
                .get("headers")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Box::pin(async move {
                let url = normalize_url(&url)
                    .ok_or_else(|| agent::ToolError::InvalidInput("url must be http(s)".into()))?;
                let method = parse_method(&method)
                    .ok_or_else(|| agent::ToolError::InvalidInput(unknown_method_msg()))?;
                let headers = sanitize_headers(&headers)?;
                fetch_url(method, &url, &body, headers).await
            })
        },
    ))
}

/// Build the `web_search` tool.
///
/// Args: `query` (required). Without a configured endpoint this queries
/// DuckDuckGo's HTML result page and extracts the organic results; with one
/// it POSTs `{query, api_key}` JSON and reads `{results: [{title, url, content}]}`.
pub(crate) fn web_search_tool(endpoint: &str, api_key: &str) -> Arc<dyn AsyncTool> {
    let endpoint = endpoint.trim().to_string();
    let api_key = api_key.trim().to_string();
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "what to search for" }
        },
        "required": ["query"],
        "additionalProperties": false
    });
    Arc::new(AsyncFunctionTool::new(
        "web_search",
        "Search the web and return the top results (title, url, snippet). \
         Use before fetching when the right page is not known yet.",
        schema,
        move |args| {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            let endpoint = endpoint.clone();
            let api_key = api_key.clone();
            Box::pin(async move {
                if query.is_empty() {
                    return Err(agent::ToolError::InvalidInput("query is required".into()));
                }
                if endpoint.is_empty() {
                    search_ddg(&query).await
                } else {
                    search_endpoint(&endpoint, &api_key, &query).await
                }
            })
        },
    ))
}

/// Accept a URL with an http(s) scheme only; anything else is not a web page.
pub(crate) fn normalize_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        Some(trimmed.to_string())
    } else if lower.starts_with("www.") {
        Some(format!("https://{trimmed}"))
    } else {
        None
    }
}

/// The methods `web_fetch` may speak, matched case-insensitively.
///
/// Connect-style verbs are left out on purpose: the tool talks to an API the
/// model already knows the URL of, and an upgraded socket is not something a
/// text tool can say anything useful about.
pub(crate) fn parse_method(raw: &str) -> Option<reqwest::Method> {
    const VERBS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
    let upper = raw.trim().to_ascii_uppercase();
    VERBS
        .into_iter()
        .find(|v| *v == upper)
        .and_then(|v| reqwest::Method::from_bytes(v.as_bytes()).ok())
}

/// The message the model reads when it names a method the tool refuses.
fn unknown_method_msg() -> String {
    "method must be one of GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS".into()
}

/// Headers the caller may not set, because they describe the connection or
/// contradict the body this tool attaches (`Host`, `Content-Length`,
/// `Transfer-Encoding`, hop-by-hop markers).
const BLOCKED_HEADERS: [&str; 6] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
];

/// One sanitized request header: a lower-cased, checked name and its value.
pub(crate) fn sanitize_headers(
    headers: &serde_json::Value,
) -> Result<Vec<(String, String)>, agent::ToolError> {
    const MAX_HEADERS: usize = 16;
    const MAX_VALUE_CHARS: usize = 4 * 1024;
    let Some(map) = headers.as_object() else {
        if headers.is_null() {
            return Ok(Vec::new());
        }
        return Err(agent::ToolError::InvalidInput(
            "headers must be an object of name -> string".into(),
        ));
    };
    if map.len() > MAX_HEADERS {
        return Err(agent::ToolError::InvalidInput(format!(
            "too many headers ({}, the ceiling is {MAX_HEADERS})",
            map.len()
        )));
    }
    let mut out = Vec::with_capacity(map.len());
    for (name, value) in map {
        let Some(value) = value.as_str() else {
            return Err(agent::ToolError::InvalidInput(format!(
                "header '{name}' must map to a string"
            )));
        };
        let lower = name.trim().to_ascii_lowercase();
        if lower.is_empty() || lower.chars().any(|c| !c.is_ascii_graphic() || c == ':') {
            return Err(agent::ToolError::InvalidInput(format!(
                "header name '{name}' is not a valid header name"
            )));
        }
        if BLOCKED_HEADERS.contains(&lower.as_str()) {
            return Err(agent::ToolError::InvalidInput(format!(
                "header '{lower}' is set by the tool itself and cannot be overridden"
            )));
        }
        if value.chars().count() > MAX_VALUE_CHARS {
            return Err(agent::ToolError::InvalidInput(format!(
                "header '{name}' value is too long (the ceiling is {MAX_VALUE_CHARS} characters)"
            )));
        }
        out.push((name.trim().to_string(), value.to_string()));
    }
    Ok(out)
}

/// Fetch `url` with any supported method and return the readable-text payload.
///
/// A body without a caller-set `Content-Type` goes out as `application/json`,
/// which is what nearly every API the model would call expects. A failed
/// status keeps its body: a 4xx/5xx answer usually carries the reason, and
/// the model can read it from the error.
async fn fetch_url(
    method: reqwest::Method,
    url: &str,
    body: &str,
    headers: Vec<(String, String)>,
) -> Result<serde_json::Value, agent::ToolError> {
    /// Characters of request body one call may carry.
    const MAX_BODY_CHARS: usize = 1024 * 1024;
    /// Characters of an error body kept for the model to read.
    const MAX_ERROR_BODY: usize = 400;

    if body.chars().count() > MAX_BODY_CHARS {
        return Err(agent::ToolError::InvalidInput(format!(
            "body is too long (the ceiling is {MAX_BODY_CHARS} characters)"
        )));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .user_agent("bos-harness/1.0 (+web_fetch)")
        .build()
        .map_err(|e| agent::ToolError::Failed(format!("client: {e}")))?;
    let mut request = client.request(method.clone(), url);
    if !body.is_empty() {
        let json_default = headers
            .iter()
            .all(|(n, _)| !n.eq_ignore_ascii_case("content-type"));
        if json_default {
            request = request.header(reqwest::header::CONTENT_TYPE, "application/json");
        }
        request = request.body(body.to_string());
    }
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|e| agent::ToolError::Failed(format!("{}: {e}", url)))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = response
        .bytes()
        .await
        .map_err(|e| agent::ToolError::Failed(format!("{url}: {e}")))?;
    if !status.is_success() {
        let excerpt: String = String::from_utf8_lossy(&body)
            .chars()
            .take(MAX_ERROR_BODY)
            .collect();
        let why = if excerpt.trim().is_empty() {
            format!("{url} answered HTTP {}", status.as_u16())
        } else {
            format!(
                "{url} answered HTTP {} — {}",
                status.as_u16(),
                excerpt.trim()
            )
        };
        return Err(agent::ToolError::Failed(why));
    }
    let body = &body[..body.len().min(MAX_BODY_BYTES)];
    let text = String::from_utf8_lossy(body);
    let is_html = content_type.contains("text/html") || content_type.contains("xml");
    let (title, content) = if is_html {
        (extract_title(&text), html_to_text(&text))
    } else {
        (String::new(), text.into_owned())
    };
    let content = clip_chars(&content, MAX_FETCH_CHARS);
    Ok(serde_json::json!({
        "url": url,
        "method": method.as_str(),
        "status": status.as_u16(),
        "content_type": content_type,
        "title": title,
        "text": content,
    }))
}

/// Search through DuckDuckGo's HTML endpoint (no key, no account).
async fn search_ddg(query: &str) -> Result<serde_json::Value, agent::ToolError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .user_agent("bos-harness/1.0 (+web_search)")
        .build()
        .map_err(|e| agent::ToolError::Failed(format!("client: {e}")))?;
    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencode(query));
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| agent::ToolError::Failed(format!("search: {e}")))?;
    if !response.status().is_success() {
        return Err(agent::ToolError::Failed(format!(
            "search answered HTTP {}",
            response.status().as_u16()
        )));
    }
    let body = response
        .text()
        .await
        .map_err(|e| agent::ToolError::Failed(format!("search body: {e}")))?;
    let results = parse_ddg_results(&body);
    Ok(serde_json::json!({
        "query": query,
        "engine": "duckduckgo",
        "results": results,
    }))
}

/// Search through a configured JSON endpoint (Tavily-style request/response).
async fn search_endpoint(
    endpoint: &str,
    api_key: &str,
    query: &str,
) -> Result<serde_json::Value, agent::ToolError> {
    let url = normalize_url(endpoint).ok_or_else(|| {
        agent::ToolError::InvalidInput("the search endpoint must be http(s)".into())
    })?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .build()
        .map_err(|e| agent::ToolError::Failed(format!("client: {e}")))?;
    let mut payload = serde_json::json!({ "query": query });
    if !api_key.is_empty() {
        payload["api_key"] = serde_json::Value::String(api_key.to_string());
    }
    let response = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| agent::ToolError::Failed(format!("search: {e}")))?;
    if !response.status().is_success() {
        return Err(agent::ToolError::Failed(format!(
            "search endpoint answered HTTP {}",
            response.status().as_u16()
        )));
    }
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|e| agent::ToolError::Failed(format!("search endpoint JSON: {e}")))?;
    // The shape is the Tavily one; anything else passes through so the model
    // can still read what came back.
    let Some(results) = value.get("results").and_then(|r| r.as_array()) else {
        return Ok(value);
    };
    let trimmed: Vec<serde_json::Value> = results
        .iter()
        .take(MAX_RESULTS)
        .map(|r| {
            serde_json::json!({
                "title": r.get("title").and_then(|t| t.as_str()).unwrap_or(""),
                "url": r.get("url").and_then(|u| u.as_str()).unwrap_or(""),
                "snippet": r
                    .get("content")
                    .or_else(|| r.get("snippet"))
                    .and_then(|s| s.as_str())
                    .unwrap_or(""),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "query": query,
        "engine": "endpoint",
        "results": trimmed,
    }))
}

/// Percent-encode everything a query string must not carry raw.
pub(crate) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The document title of an HTML page, without its tags.
pub(crate) fn extract_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(start) = lower.find("<title") else {
        return String::new();
    };
    let Some(open) = lower[start..].find('>') else {
        return String::new();
    };
    let from = start + open + 1;
    let Some(end) = lower[from..].find("</title") else {
        return String::new();
    };
    let title = html_to_text(&html[from..from + end]);
    title.trim().to_string()
}

/// Reduce HTML to readable text: scripts, styles and tags gone, block
/// boundaries become newlines, the common entities decoded, whitespace
/// collapsed. Pure, so the search parser and tests share it.
pub(crate) fn html_to_text(html: &str) -> String {
    let no_script = strip_block(html, "script");
    let no_style = strip_block(&no_script, "style");
    let no_head = strip_block(&no_style, "head");
    let mut out = String::with_capacity(no_head.len());
    let mut i = 0;
    while i < no_head.len() {
        let c = no_head[i..].chars().next().unwrap_or('<');
        if c == '<' {
            if no_head[i..].starts_with("<!--") {
                match no_head[i..].find("-->") {
                    Some(end) => i += end + 3,
                    None => break, // unterminated comment: the rest is not text
                }
                continue;
            }
            match no_head[i..].find('>') {
                Some(end) => {
                    if block_boundary(&no_head[i..i + end + 1]) {
                        out.push('\n');
                    }
                    i += end + 1;
                }
                None => break, // truncated tag: the rest is markup, not text
            }
        } else {
            out.push(c);
            i += c.len_utf8();
        }
    }
    decode_entities(&collapse_space(&out))
}

/// Whether a tag starts or ends a block, i.e. renders as a line break.
fn block_boundary(tag: &str) -> bool {
    let Some(after_open) = tag.strip_prefix('<') else {
        return false;
    };
    let name: String = after_open
        .trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    matches!(name.as_str(), "br" | "li" | "p" | "div" | "tr")
        || (name.len() == 2 && name.starts_with('h') && name.as_bytes()[1].is_ascii_digit())
}

/// Drop every `<name …>…</name>` region, case-insensitively.
///
/// The name must be followed by a space, `>` or `/`, so a `<head>` scan never
/// swallows a `<header>` element, and an unterminated block drops the tail.
fn strip_block(html: &str, name: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        let Some(start) = find_tag(rest, name, false) else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(close) = find_tag(after, name, true) else {
            return out; // unterminated: drop the tail
        };
        match after[close..].find('>') {
            Some(gt) => rest = &after[close + gt + 1..],
            None => return out,
        }
    }
}

/// Find `<name …>` (opening) or `</name>` (closing) at a tag boundary.
fn find_tag(html: &str, name: &str, closing: bool) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    loop {
        let pat = if closing {
            format!("</{name}")
        } else {
            format!("<{name}")
        };
        let i = lower[from..].find(&pat)? + from;
        let after = &lower[i + pat.len()..];
        let boundary = after
            .chars()
            .next()
            .map(|c| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(true);
        if boundary {
            return Some(i);
        }
        from = i + pat.len();
    }
}

/// Decode the entities an HTML page actually leans on; unknown ones pass through.
pub(crate) fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let semi = tail.find(';').unwrap_or(tail.len());
        let entity = &tail[1..semi.min(tail.len())];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            e if e.len() >= 2 && e.as_bytes()[0] == b'#' => {
                let code = e[1..].trim_start_matches(['x', 'X']);
                u32::from_str_radix(code, if e[1..].len() != code.len() { 16 } else { 10 })
                    .ok()
                    .and_then(char::from_u32)
            }
            _ => None,
        };
        match decoded {
            Some(c) if semi < tail.len() => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            _ => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Collapse runs of whitespace, keeping single line breaks where blocks were.
fn collapse_space(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    let mut newline = false;
    for c in s.chars() {
        if c == '\n' {
            newline = true;
            space = false;
            continue;
        }
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if newline {
            out.push('\n');
        } else if space {
            out.push(' ');
        }
        newline = false;
        space = false;
        out.push(c);
    }
    if newline {
        out.push('\n');
    }
    out.trim().to_string()
}

/// Clip a string on character boundaries, marking the cut.
pub(crate) fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}\n… [truncated]")
}

/// One organic search result.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct SearchResult {
    /// The result title.
    pub title: String,
    /// The target URL (DuckDuckGo redirects resolved).
    pub url: String,
    /// The snippet under the title.
    pub snippet: String,
}

/// Extract the organic results from a DuckDuckGo HTML answer page.
///
/// The page is a stream of anchors: one with `class="result__a"` opens a row
/// (its text is the title, its href the target), and the next with
/// `class="result__snippet"` attaches the excerpt to the row before it.
/// Walking all anchors in order — instead of jumping between class markers —
/// keeps titles and snippets paired however the page nests them, and a row
/// without a snippet costs only its excerpt.
pub(crate) fn parse_ddg_results(html: &str) -> Vec<SearchResult> {
    let mut results: Vec<SearchResult> = Vec::new();
    for (class, href, text) in anchors(html) {
        let is_title = class.split_whitespace().any(|c| c == "result__a");
        let is_snippet = class.split_whitespace().any(|c| c == "result__snippet");
        if is_title {
            if results.len() >= MAX_RESULTS {
                break;
            }
            results.push(SearchResult {
                title: text,
                url: resolve_ddg_href(&href),
                snippet: String::new(),
            });
        } else if is_snippet {
            if let Some(last) = results.last_mut() {
                if last.snippet.is_empty() {
                    last.snippet = text;
                }
            }
        }
    }
    results
}

/// Every `<a …>…</a>` on the page in order, as `(class, href, inner text)`.
fn anchors(html: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    loop {
        let Some(rel) = find_anchor_open(rest) else {
            return out;
        };
        let tag_end = match rest[rel..].find('>') {
            Some(e) => rel + e,
            None => return out, // truncated tag: the rest is markup
        };
        let tag = &rest[rel..tag_end + 1];
        let after = &rest[tag_end + 1..];
        let Some(close) = after.to_ascii_lowercase().find("</a>") else {
            return out; // unterminated anchor: the tail is not a result
        };
        let text = html_to_text(&after[..close]).trim().to_string();
        let class = attr_value(tag, "class").unwrap_or_default();
        let href = attr_value(tag, "href").unwrap_or_default();
        out.push((class, href, text));
        rest = &after[close + 4..];
    }
}

/// The offset of the next real anchor opening, skipping look-alikes such as
/// `<abbr>`: `<a` must be followed by whitespace or the tag's `>`.
fn find_anchor_open(html: &str) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    loop {
        let i = lower[from..].find("<a")? + from;
        match lower[i + 2..].chars().next() {
            Some(c) if c.is_whitespace() || c == '>' => return Some(i),
            _ => from = i + 2,
        }
    }
}

/// One attribute's value inside a tag string.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let pat = format!("{name}=\"");
    let i = lower.find(&pat)?;
    let from = i + pat.len();
    let end = tag[from..].find('"')? + from;
    Some(tag[from..end].to_string())
}

/// Turn a DuckDuckGo redirect (`//duckduckgo.com/l/?uddg=<encoded>&…`) into
/// the page it points at; plain links pass through untouched.
pub(crate) fn resolve_ddg_href(href: &str) -> String {
    let trimmed = href.trim();
    if let Some(pos) = trimmed.find("uddg=") {
        let encoded = &trimmed[pos + 5..];
        let end = encoded.find('&').unwrap_or(encoded.len());
        if let Ok(decoded) = percent_decode(&encoded[..end]) {
            return decoded;
        }
    }
    if let Some(rest) = trimmed.strip_prefix("//") {
        return format!("https://{rest}");
    }
    trimmed.to_string()
}

/// Minimal percent-decoding for redirect parameters.
fn percent_decode(s: &str) -> Result<String, ()> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|_| ())?;
            let byte = u8::from_str_radix(hex, 16).map_err(|_| ())?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_parse_case_insensitively_and_connect_verbs_refuse() {
        for verb in [
            "GET", "get", "Post", "PUT", "patch", "DELETE", "head", "OPTIONS",
        ] {
            assert!(parse_method(verb).is_some(), "{verb} must parse");
        }
        assert_eq!(parse_method("get").unwrap(), reqwest::Method::GET);
        assert!(parse_method("CONNECT").is_none());
        assert!(parse_method("TRACE").is_none());
        assert!(parse_method("FOO").is_none());
        assert!(parse_method("").is_none());
        assert!(
            parse_method("  put  ").is_some(),
            "surrounding space is trimmed"
        );
    }

    #[test]
    fn headers_sanitize_and_block_connection_level_names() {
        let headers = serde_json::json!({
            "Authorization": "Bearer t0k3n",
            "X-Custom": "v"
        });
        let out = sanitize_headers(&headers).expect("plain headers pass");
        assert_eq!(
            out,
            vec![
                ("Authorization".to_string(), "Bearer t0k3n".to_string()),
                ("X-Custom".to_string(), "v".to_string()),
            ]
        );

        assert!(sanitize_headers(&serde_json::Value::Null)
            .unwrap()
            .is_empty());
        assert!(sanitize_headers(&serde_json::json!({})).unwrap().is_empty());
        assert!(
            sanitize_headers(&serde_json::json!([1])).is_err(),
            "not an object"
        );
        assert!(
            sanitize_headers(&serde_json::json!({"X": 3})).is_err(),
            "values must be strings"
        );
        for blocked in [
            "Host",
            "content-length",
            "Transfer-Encoding",
            "connection",
            "Upgrade",
            "TE",
        ] {
            assert!(
                sanitize_headers(&serde_json::json!({ blocked: "x" })).is_err(),
                "{blocked} must be refused"
            );
        }
        assert!(
            sanitize_headers(&serde_json::json!({"bad name\n": "x"})).is_err(),
            "control characters are not a header name"
        );
        let many: serde_json::Value = (0..17)
            .map(|i| (format!("h{i}"), serde_json::Value::String("v".into())))
            .collect::<serde_json::Map<String, _>>()
            .into();
        assert!(sanitize_headers(&many).is_err(), "past the ceiling refused");
    }

    #[test]
    fn only_http_urls_pass() {
        assert_eq!(
            normalize_url("https://example.com/x"),
            Some("https://example.com/x".into())
        );
        assert_eq!(
            normalize_url("http://example.com"),
            Some("http://example.com".into())
        );
        assert_eq!(
            normalize_url("www.example.com"),
            Some("https://www.example.com".into()),
            "a bare www. host is the one prefix worth completing"
        );
        assert_eq!(normalize_url("file:///etc/passwd"), None);
        assert_eq!(normalize_url("ftp://x"), None);
        assert_eq!(normalize_url("javascript:alert(1)"), None);
        assert_eq!(normalize_url(""), None);
    }

    #[test]
    fn html_reduces_to_text_and_keeps_block_boundaries() {
        let html = concat!(
            "<html><head><title>My & Page</title>",
            "<style>.x{color:red}</style><script>if(a<b){}</script></head>",
            "<body><h1>Hello</h1><p>One <two> three&nbsp;words</p>",
            "<div>second<br>line</div><!-- a comment <div>hidden</div> -->tail</body></html>"
        );
        let text = html_to_text(html);
        assert!(text.contains("Hello"), "{text}");
        assert!(
            text.contains("One three words"),
            "the tag vanishes, the words stay: {text}"
        );
        assert!(text.contains("second\nline"), "{text}");
        assert!(!text.contains("color:red"), "styles must go: {text}");
        assert!(!text.contains("if(a"), "scripts must go: {text}");
        assert!(!text.contains("hidden"), "comments must go: {text}");
        assert!(text.contains("tail"), "{text}");
    }

    #[test]
    fn a_header_is_not_a_head_block() {
        let html = "<header><nav>Menu</nav></header><p>Body</p>";
        let text = html_to_text(html);
        assert!(
            text.contains("Menu"),
            "<header> must survive a <head> strip: {text}"
        );
    }

    #[test]
    fn entities_decode_and_unknowns_pass_through() {
        assert_eq!(
            decode_entities("a & b <x> &quot;q&quot;"),
            "a & b <x> \"q\"",
        );
        assert_eq!(decode_entities("&#65;&#x42;"), "AB");
        assert_eq!(
            decode_entities("100 & 200"),
            "100 & 200",
            "a bare & is not an entity"
        );
    }

    #[test]
    fn fetch_text_is_clipped_on_characters() {
        let long = "你".repeat(30);
        let cut = clip_chars(&long, 10);
        let head: String = long.chars().take(10).collect();
        assert!(cut.starts_with(&head), "{cut}");
        assert!(cut.contains("truncated"), "{cut}");
        assert!(
            cut.chars().count() <= 10 + 20,
            "the marker is the only overshoot: {cut}"
        );
        assert_eq!(clip_chars("short", 10), "short");
    }

    const DDG_PAGE: &str = r#"<html><body>
    <div class="results">
      <div class="result">
        <h2><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&rut=abc">First result</a></h2>
        <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa">The & first snippet</a>
      </div>
      <div class="result">
        <h2><a rel="nofollow" class="result__a" href="https://direct.example.com/b">Second "quoted"</a></h2>
        <a class="result__snippet" href="https://direct.example.com/b">plain snippet</a>
      </div>
      <div class="result">
        <h2><a rel="nofollow" class="result__a" href="https://bare.example.com/c">Bare</a></h2>
      </div>
    </div></body></html>"#;

    #[test]
    fn ddg_results_carry_resolved_urls_titles_and_snippets() {
        let rows = parse_ddg_results(DDG_PAGE);
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[0].title, "First result");
        assert_eq!(
            rows[0].url, "https://example.com/a",
            "the redirect resolves"
        );
        assert_eq!(rows[0].snippet, "The & first snippet");
        assert_eq!(
            rows[1].url, "https://direct.example.com/b",
            "plain links pass through"
        );
        assert_eq!(rows[1].title, "Second \"quoted\"");
        assert_eq!(
            rows[2].snippet, "",
            "a row without a snippet keeps its place"
        );
    }

    #[test]
    fn ddg_parse_of_a_page_without_results_is_empty_not_stuck() {
        assert!(parse_ddg_results("<html><body>nothing</body></html>").is_empty());
        // A title anchor with no closing tag must not loop forever.
        let truncated = r#"<a class="result__a" href="https://x">unterminated"#;
        let rows = parse_ddg_results(truncated);
        assert!(rows.len() <= 1, "{rows:?}");
    }

    #[test]
    fn urlencoding_escapes_query_characters() {
        assert_eq!(urlencode("rust lang"), "rust%20lang");
        assert_eq!(urlencode("a&b=c/d"), "a%26b%3Dc%2Fd");
        assert_eq!(urlencode("safe-._~"), "safe-._~");
        assert_eq!(urlencode("你好"), "%E4%BD%A0%E5%A5%BD");
    }

    #[test]
    fn titles_extract_without_their_tags() {
        assert_eq!(extract_title("<title>Hi <b>there</b></title>"), "Hi there");
        assert_eq!(extract_title("<TITLE>Upper</TITLE>"), "Upper");
        assert_eq!(extract_title("<html><body>x</body></html>"), "");
    }

    /// Serve one request on a loopback socket: read what the client sent and
    /// answer a fixed JSON 200, handing the raw request back for assertions.
    fn serve_one() -> (std::net::SocketAddr, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).expect("read");
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                let Some(header_end) = text.find("\r\n\r\n") else {
                    continue;
                };
                let length = text
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if raw.len() >= header_end + 4 + length {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&raw).to_string();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}",
                )
                .expect("write response");
            request
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn a_post_sends_its_json_body_and_custom_headers() {
        let (addr, server) = serve_one();
        let url = format!("http://{addr}/echo");
        let payload = fetch_url(
            reqwest::Method::POST,
            &url,
            r#"{"name":"bos"}"#,
            vec![("X-Trace".to_string(), "t-1".to_string())],
        )
        .await
        .expect("the POST succeeds");
        let request = server.join().expect("server thread");
        assert!(request.starts_with("POST /echo HTTP/1.1"), "{request}");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("content-type: application/json"),
            "a body without a content type defaults to JSON: {request}"
        );
        assert!(request.contains("x-trace: t-1"), "{request}");
        assert!(request.contains(r#"{"name":"bos"}"#), "{request}");

        let text = payload["text"].as_str().expect("text field");
        assert_eq!(text, r#"{"ok":true}"#);
        assert_eq!(payload["status"], 200);
        assert_eq!(payload["method"], "POST");
    }

    #[tokio::test]
    async fn a_caller_content_type_wins_over_the_json_default() {
        let (addr, server) = serve_one();
        let url = format!("http://{addr}/form");
        fetch_url(
            reqwest::Method::PUT,
            &url,
            "a=1&b=2",
            vec![(
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            )],
        )
        .await
        .expect("the PUT succeeds");
        let request = server.join().expect("server thread");
        assert!(request.starts_with("PUT /form HTTP/1.1"), "{request}");
        let lower = request.to_ascii_lowercase();
        assert!(
            lower.contains("content-type: application/x-www-form-urlencoded"),
            "{request}"
        );
        assert!(
            !lower.contains("content-type: application/json"),
            "{request}"
        );
        assert!(request.contains("a=1&b=2"), "{request}");
    }

    #[tokio::test]
    async fn a_failed_status_keeps_its_body_for_the_model_to_read() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            stream
                .write_all(
                    b"HTTP/1.1 422 Unprocessable\r\nContent-Type: application/json\r\nContent-Length: 20\r\n\r\n{\"error\":\"bad name\"}",
                )
                .expect("write");
        });
        let url = format!("http://{addr}/fail");
        let err = fetch_url(reqwest::Method::GET, &url, "", Vec::new())
            .await
            .expect_err("a 422 is an error");
        server.join().expect("server thread");
        let message = err.to_string();
        assert!(message.contains("422"), "{message}");
        assert!(
            message.contains("bad name"),
            "the body explains the failure: {message}"
        );
    }

    #[tokio::test]
    async fn a_body_past_the_ceiling_is_refused_before_the_wire() {
        let big = "x".repeat(1024 * 1024 + 1);
        // The ceiling is checked before the client is even built, so the
        // unroutable port below is never contacted.
        let err = fetch_url(
            reqwest::Method::POST,
            "http://127.0.0.1:9/nothing",
            &big,
            Vec::new(),
        )
        .await
        .expect_err("an oversized body is refused");
        assert!(err.to_string().contains("too long"), "{err}");
    }
}
