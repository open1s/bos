//! On-demand tool discovery: one search tool that reveals schemas in place.
//!
//! A machine accumulates tools - an MCP server alone can contribute hundreds -
//! and every one of them costs prompt bytes on every step when it is
//! advertised. A tool marked `deferred` (see [`react::tool::registry::Tool`])
//! stays registered and stays callable; it is missing from the advertised
//! array only. The model asks `find_tool`, whose reported definitions join the
//! next request's tool array, and then calls the tool by its own name.
//!
//! Search is LEXICAL, and that is a promise worth stating: a tool is found by
//! the words in its name and description, so a synonym its description does not
//! use will not reach it. A tool that must answer "find the code" has to say
//! "code" somewhere; the recall fixture below pins the shortlist for the queries
//! a reader actually types.
//!
//! Discovery is deliberately NOT a capability boundary: a deferred tool stays
//! registered and callable. What is withheld is the definition, not the
//! ability, and policy still sees the real tool name on the direct call.

use crate::tools::{FunctionTool, ToolRegistry};
use react::llm::LlmTool;
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

/// The search tool's name.
pub const FIND_TOOL: &str = "find_tool";

/// The description both the definition and the tool carry.
///
/// It promises the full argument schema, because a search that described a
/// tool's parameters in shorthand left the model guessing at the JSON that
/// actually runs it.
const FIND_DESCRIPTION: &str = "Find a tool you were not given directly, by what you need to do or by its exact name. Returns each match's description and full JSON Schema for its arguments. The reported tools are added to your tool list, so call them directly by name with args matching that schema. Tools already in your list are not returned. Use it before concluding a capability is missing.";

/// Most tools one search describes in full.
const FIND_DESCRIBED: usize = 3;
/// The longest description one result keeps.
const FIND_DESCRIPTION_CHARS: usize = 200;
/// The byte budget for one search result.
///
/// A result now carries each shortlisted tool's whole schema rather than a
/// one-line rendering, so the budget is priced against the JSON the model
/// actually has to send.
const FIND_BUDGET: usize = 4_096;
/// Most names a result lists without describing them.
const FIND_NAMES: usize = 80;

/// The model-facing definition of the search tool.
pub fn find_tool_definition() -> LlmTool {
    LlmTool::function(FIND_TOOL, FIND_DESCRIPTION, find_tool_schema())
}

/// The argument schema of the search tool.
pub fn find_tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "What you need to do, in words. Omit to list every tool name."
            },
            "name": {
                "type": "string",
                "description": "An exact tool name; returns just that one, with its full argument schema."
            },
            "limit": {
                "type": "integer",
                "description": "How many tools to describe in full, schema included (default 3, max 5)."
            }
        }
    })
}

/// The skill search tool's name.
pub const FIND_SKILL: &str = "find_skill";

/// The description both the definition and the tool carry.
const FIND_SKILL_DESCRIPTION: &str = "Check this before starting work a skill might cover: find a skill by what you need to do. Returns the best matches with their descriptions; read one with load_skill. Call it with no query to list every skill name.";

/// Most skills one search describes in full.
const FIND_SKILL_DESCRIBED: usize = 3;
/// Most skill names a result lists without describing them.
const FIND_SKILL_NAMES: usize = 100;
/// The byte budget for one skill search.
const FIND_SKILL_BUDGET: usize = 2_048;

/// The model-facing definition of the skill search.
pub fn find_skill_definition() -> LlmTool {
    LlmTool::function(FIND_SKILL, FIND_SKILL_DESCRIPTION, find_skill_schema())
}

/// The argument schema of the skill search.
pub fn find_skill_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "What you need to do, in words. Omit to list every skill name."
            },
            "name": {
                "type": "string",
                "description": "An exact skill name; returns just that one."
            }
        }
    })
}

/// One skill a search can offer: what the catalogue used to carry inline.
struct SkillEntry {
    name: String,
    description: String,
    dir: String,
}

/// Build the skill search over the agent's base skills and its live overlay.
///
/// A package's skills arrive after the build, so the live half is read on every
/// call; the base half is a snapshot, because those skills are fixed for the
/// life of this engine.
pub fn find_skill(
    base: Vec<crate::skills::SkillContent>,
    live: Arc<RwLock<Vec<crate::skills::SkillContent>>>,
) -> FunctionTool {
    FunctionTool::new(
        FIND_SKILL,
        FIND_SKILL_DESCRIPTION,
        find_skill_schema(),
        move |args: &Value| {
            let guard = live.read().ok();
            let entries = skill_entries(&base, guard.as_deref().map(Vec::as_slice));
            Ok(search_skills(&entries, args))
        },
    )
}

/// The model-invocable skills, base first with the live overlay on top.
fn skill_entries(
    base: &[crate::skills::SkillContent],
    live: Option<&[crate::skills::SkillContent]>,
) -> Vec<SkillEntry> {
    let mut entries: Vec<SkillEntry> = Vec::new();
    for skill in base.iter().chain(live.into_iter().flatten()) {
        if skill.metadata.user_only {
            continue;
        }
        entries.push(SkillEntry {
            name: skill.metadata.name.clone(),
            description: skill.metadata.description.clone(),
            dir: skill.skill_dir.to_string_lossy().to_string(),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries.dedup_by(|a, b| a.name == b.name);
    entries
}

/// One skill search: an exact name, a ranked query, or every name.
fn search_skills(entries: &[SkillEntry], args: &Value) -> Value {
    if let Some(name) = args.get("name").and_then(Value::as_str) {
        return match entries.iter().find(|entry| entry.name == name) {
            Some(entry) => json!({
                "name": entry.name,
                "description": entry.description,
                "skill_dir": entry.dir,
                "hint": "Call load_skill with this name to read its instructions.",
            }),
            None => json!({
                "error": format!("No skill named '{name}'. Search by what you need instead."),
            }),
        };
    }

    let query = args.get("query").and_then(Value::as_str).unwrap_or("");
    if query.trim().is_empty() {
        let names: Vec<&str> = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .take(FIND_SKILL_NAMES)
            .collect();
        return json!({
            "total": entries.len(),
            "names": names,
            "hint": "Call find_skill with a query or a name, then load_skill to read one.",
        });
    }

    let words = words(query);
    let mut ranked: Vec<(i64, &SkillEntry)> = entries
        .iter()
        .map(|entry| (score_text(&words, &entry.name, &entry.description), entry))
        .filter(|(score, _)| *score > 0)
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));

    let mut described = Vec::new();
    let mut spent = 0usize;
    for (_, entry) in ranked.iter().take(FIND_SKILL_DESCRIBED) {
        spent += entry.name.len() + entry.description.len();
        if spent > FIND_SKILL_BUDGET && !described.is_empty() {
            break;
        }
        described.push(json!({
            "name": entry.name,
            "description": clip(&entry.description, FIND_DESCRIPTION_CHARS),
        }));
    }
    let described_names: Vec<String> = described
        .iter()
        .filter_map(|value| value.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let names_only: Vec<&str> = ranked
        .iter()
        .map(|(_, entry)| entry.name.as_str())
        .filter(|name| !described_names.iter().any(|described| described == name))
        .take(FIND_SKILL_NAMES)
        .collect();

    json!({
        "total": entries.len(),
        "skills": described,
        "other_names": names_only,
        "hint": "Call load_skill with one of these names to read its instructions.",
    })
}

/// The host's withheld-name policy, snapshotted when the search was built.
///
/// A tool that withholds itself (`Tool::deferred`) and an MCP tool travel with
/// their own mark, so those are read at call time; this carries the names the
/// host listed explicitly, which a tool cannot know about itself.
#[derive(Clone, Default)]
pub struct Withheld {
    exact: Arc<Vec<String>>,
    prefixes: Arc<Vec<String>>,
}

impl Withheld {
    /// Snapshot a host's withheld names and name prefixes.
    pub fn new(exact: Vec<String>, prefixes: Vec<String>) -> Self {
        Self {
            exact: Arc::new(exact),
            prefixes: Arc::new(prefixes),
        }
    }

    fn matches(&self, name: &str) -> bool {
        self.exact.iter().any(|exact| exact == name)
            || self
                .prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix.as_str()))
    }
}

/// What the search may offer: the tools the request does NOT carry.
///
/// A tool the model already has is not a search result. Offering one would
/// invite a reverse lookup for something a direct call does better, and it would
/// spend the shortlist on answers the model can already see.
struct Catalogue<'a> {
    registry: Option<&'a ToolRegistry>,
    live: Option<&'a ToolRegistry>,
    withheld: Withheld,
}

/// What an exact-name lookup found.
enum Lookup {
    /// Withheld from the request, with its description and schema.
    Withheld(String, Value),
    /// Registered, but already in the model's own tool list.
    Advertised,
    /// Not registered at all.
    Missing,
}

impl Catalogue<'_> {
    fn is_withheld(&self, name: &str, deferred: bool, mcp: bool) -> bool {
        deferred || mcp || self.withheld.matches(name)
    }

    fn entries(&self) -> Vec<(String, String, Value)> {
        let mut out: Vec<(String, String, Value)> = Vec::new();
        for registry in [self.registry, self.live].into_iter().flatten() {
            for (name, tool) in registry.iter() {
                if !self.is_withheld(name, tool.deferred(), registry.is_mcp_tool(name)) {
                    continue;
                }
                out.push((name.clone(), tool.description(), tool.json_schema()));
            }
            for name in registry.async_tool_names() {
                if let Some(tool) = registry.get_async(&name) {
                    if !self.is_withheld(&name, tool.deferred(), registry.is_mcp_tool(&name)) {
                        continue;
                    }
                    out.push((name.clone(), tool.description(), tool.json_schema()));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.dedup_by(|a, b| a.0 == b.0);
        out
    }

    /// Every withheld tool's name, sorted.
    fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for registry in [self.registry, self.live].into_iter().flatten() {
            for (name, tool) in registry.iter() {
                if self.is_withheld(name, tool.deferred(), registry.is_mcp_tool(name)) {
                    out.push(name.clone());
                }
            }
            for name in registry.async_tool_names() {
                let is_withheld = registry.get_async(&name).is_some_and(|tool| {
                    self.is_withheld(&name, tool.deferred(), registry.is_mcp_tool(&name))
                });
                if is_withheld {
                    out.push(name);
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// The registered name a reader's word refers to, when that is unambiguous.
    ///
    /// A skill or a command names a tool the way its author says it - "call
    /// design" - while the registry exposes the package's tool under a namespace
    /// prefix ("extdev__design"). An exact-match-only lookup turns that into a
    /// wasted round trip: the model asks for a name that exists, is told it does
    /// not, and has to search for it. A bare word resolves to the one withheld
    /// tool whose last segment is that word; two candidates stay unresolved,
    /// because guessing one would run the wrong tool.
    fn resolve(&self, requested: &str) -> Option<String> {
        let requested = requested.trim();
        if requested.is_empty() {
            return None;
        }
        let names = self.names();
        if names.iter().any(|name| name == requested) {
            return Some(requested.to_string());
        }
        let mut hit: Option<&String> = None;
        for name in &names {
            if name == requested {
                continue;
            }
            let last = name.rsplit("__").next().unwrap_or(name);
            let mcp_last = name.rsplit('_').next().unwrap_or(name);
            if last != requested && mcp_last != requested {
                continue;
            }
            if hit.is_some() {
                return None;
            }
            hit = Some(name);
        }
        hit.cloned()
    }

    fn get(&self, name: &str) -> Lookup {
        for registry in [self.registry, self.live].into_iter().flatten() {
            let found = registry
                .get(name)
                .map(|tool| (tool.deferred(), tool.description(), tool.json_schema()))
                .or_else(|| {
                    registry
                        .get_async(name)
                        .map(|tool| (tool.deferred(), tool.description(), tool.json_schema()))
                });
            if let Some((deferred, description, schema)) = found {
                return if self.is_withheld(name, deferred, registry.is_mcp_tool(name)) {
                    Lookup::Withheld(description, schema)
                } else {
                    Lookup::Advertised
                };
            }
        }
        Lookup::Missing
    }
}

/// Build the search tool over the agent's registry and its live overlay.
pub fn find_tool(
    registry: Option<Arc<ToolRegistry>>,
    live: Arc<RwLock<ToolRegistry>>,
    withheld: Withheld,
) -> FunctionTool {
    FunctionTool::new(
        FIND_TOOL,
        FIND_DESCRIPTION,
        find_tool_schema(),
        move |args: &Value| {
            let guard = live.read().ok();
            let catalogue = Catalogue {
                registry: registry.as_deref(),
                live: guard.as_deref(),
                withheld: withheld.clone(),
            };
            Ok(search(&catalogue, args))
        },
    )
}

/// One search: an exact name, a ranked query, or the whole list of names.
fn search(catalogue: &Catalogue<'_>, args: &Value) -> Value {
    if let Some(requested) = args.get("name").and_then(Value::as_str) {
        // A bare word a skill uses ("design") is resolved to the unique
        // namespaced tool it names ("extdev__design"); the answer always
        // carries the registered name, so the direct call runs.
        let canonical = catalogue
            .resolve(requested)
            .unwrap_or_else(|| requested.to_string());
        let name = canonical.as_str();
        return match catalogue.get(name) {
            Lookup::Withheld(description, schema) => json!({
                "name": name,
                "description": description,
                "parameters": schema,
                "hint": format!(
                    "Call {name} directly with arguments matching parameters; it is now in your tool list."
                ),
            }),
            Lookup::Advertised => json!({
                "error": format!(
                    "'{name}' is already in your tool list - call it directly."
                ),
            }),
            Lookup::Missing => {
                // A near miss is the second wasted round trip: name what does
                // exist, so the model can correct itself in the same step.
                let names = catalogue.names();
                let needle = requested.to_ascii_lowercase();
                let near: Vec<&str> = names
                    .iter()
                    .filter(|candidate| {
                        let candidate = candidate.to_ascii_lowercase();
                        needle.len() > 2
                            && (candidate.contains(&needle) || needle.contains(candidate.as_str()))
                    })
                    .map(String::as_str)
                    .take(8)
                    .collect();
                if near.is_empty() {
                    json!({
                        "error": format!("No tool named '{requested}'. Search by what you need instead."),
                    })
                } else {
                    json!({
                        "error": format!("No tool named '{requested}'. Search by what you need instead."),
                        "near_names": near,
                    })
                }
            }
        };
    }

    let entries = catalogue.entries();
    let query = args.get("query").and_then(Value::as_str).unwrap_or("");
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(FIND_DESCRIBED)
        .clamp(1, 5);

    if query.trim().is_empty() {
        // A listing is the escape hatch when the model cannot name what it
        // needs: every tool's name, nothing else.
        let names: Vec<&str> = entries
            .iter()
            .map(|entry| entry.0.as_str())
            .take(FIND_NAMES)
            .collect();
        return json!({
            "total": entries.len(),
            "names": names,
            "hint": "Call find_tool with a query or a name to get a tool's arguments; reported tools join your tool list.",
        });
    }

    let words = words(query);
    let mut ranked: Vec<(i64, &(String, String, Value))> = entries
        .iter()
        .map(|entry| (score(&words, entry), entry))
        .filter(|(score, _)| *score > 0)
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1 .0.cmp(&b.1 .0)));

    let mut described = Vec::new();
    let mut spent = 0usize;
    for (_, (name, description, schema)) in ranked.iter().take(limit) {
        // Price the schema itself, not a rendering of it: the JSON below is
        // what the model reads, and parameter definitions are the reason it
        // asked.
        spent += name.len() + description.len() + schema.to_string().len();
        if spent > FIND_BUDGET && !described.is_empty() {
            break;
        }
        described.push(json!({
            "name": name,
            "description": clip(description, FIND_DESCRIPTION_CHARS),
            "parameters": schema,
        }));
    }

    let described_names: Vec<String> = described
        .iter()
        .filter_map(|value| value.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let names_only: Vec<&str> = ranked
        .iter()
        .map(|(_, entry)| entry.0.as_str())
        .filter(|name| !described_names.iter().any(|described| described == name))
        .take(FIND_NAMES)
        .collect();

    let mut result = json!({
        "total": entries.len(),
        "tools": described,
        "other_names": names_only,
        "hint": "Call the reported tools directly by name with args matching parameters. Call find_tool with {name: \"<tool>\"} for the full schema of any name in other_names.",
    });
    if result.to_string().len() > FIND_BUDGET + FIND_BUDGET / 2 {
        if let Some(object) = result.as_object_mut() {
            object.remove("other_names");
        }
    }
    result
}

/// Shorter than `text`, cut at a character boundary.
fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

/// The words a query is asking with.
fn words(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() > 2)
        .map(|word| word.to_ascii_lowercase())
        .collect()
}

/// How well one tool answers the query. A name match counts double: the model
/// usually knows roughly what it wants to do, and a tool whose NAME says so is
/// the better answer than one whose long description happens to mention it.
fn score(words: &[String], entry: &(String, String, Value)) -> i64 {
    score_text(words, &entry.0, &entry.1)
}

/// The scoring rule both searches share: a name match counts double, because a
/// tool or skill whose NAME says it is the better answer than one whose long
/// description happens to mention the word.
fn score_text(words: &[String], name: &str, description: &str) -> i64 {
    let name = name.to_ascii_lowercase();
    let description = description.to_ascii_lowercase();
    words.iter().fold(0, |total, word| {
        total
            + i64::from(name.contains(word.as_str())) * 2
            + i64::from(description.contains(word.as_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, description: &str) -> (String, String, Value) {
        (
            name.to_string(),
            description.to_string(),
            json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        )
    }

    fn registry_with(tools: &[(&str, &str)]) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for (name, description) in tools {
            let tool = FunctionTool::new(
                name,
                description,
                json!({"type": "object", "properties": {"x": {"type": "string"}}}),
                |_| Ok(json!({"ok": true})),
            );
            let name = name.to_string();
            registry.register(Arc::new(tool)).unwrap();
            // A fixture stands for the tools a request does not carry: the
            // search sees those, and only those.
            registry.mark_mcp_tool(&name);
        }
        registry
    }

    fn skill(name: &str, description: &str, user_only: bool) -> crate::skills::SkillContent {
        let mut metadata = crate::skills::SkillMetadata::new(
            name.to_string(),
            description.to_string(),
            std::path::PathBuf::from(format!("/skills/{name}/SKILL.md")),
        );
        metadata.user_only = user_only;
        crate::skills::SkillContent {
            metadata,
            instructions: format!("instructions for {name}"),
            skill_dir: std::path::PathBuf::from(format!("/skills/{name}")),
        }
    }

    #[test]
    fn a_skill_search_shortlists_what_the_query_means() {
        let entries = skill_entries(
            &[
                skill("git-commit", "Write a clear commit message", false),
                skill("sec-review", "Review a change for security", false),
                skill("docs", "Write documentation", false),
            ],
            None,
        );
        let result = search_skills(&entries, &json!({"query": "commit message"}));
        assert_eq!(result["skills"][0]["name"], "git-commit");
        assert!(result["skills"][0]["description"]
            .as_str()
            .unwrap()
            .contains("commit"));
    }

    #[test]
    fn a_skill_search_without_words_lists_every_name() {
        let entries = skill_entries(
            &[
                skill("alpha", "first", false),
                skill("beta", "second", false),
            ],
            None,
        );
        let result = search_skills(&entries, &json!({}));
        let names = result["names"].as_array().unwrap();
        assert_eq!(names.len(), 2);
        assert_eq!(result["total"], 2);
    }

    #[test]
    fn a_user_only_skill_is_never_found() {
        let entries = skill_entries(
            &[
                skill("mine", "reserved for the reader", true),
                skill("shared", "anyone may load it", false),
            ],
            None,
        );
        let result = search_skills(&entries, &json!({}));
        let names: Vec<&str> = result["names"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|name| name.as_str())
            .collect();
        assert_eq!(names, vec!["shared"]);
    }

    #[test]
    fn an_exact_skill_name_points_at_the_loader() {
        let entries = skill_entries(
            &[skill("git-commit", "Write a commit message", false)],
            None,
        );
        let result = search_skills(&entries, &json!({"name": "git-commit"}));
        assert_eq!(result["name"], "git-commit");
        assert!(result["hint"].as_str().unwrap().contains("load_skill"));
    }

    #[test]
    fn an_unknown_skill_name_says_to_search() {
        let entries = skill_entries(
            &[skill("git-commit", "Write a commit message", false)],
            None,
        );
        let result = search_skills(&entries, &json!({"name": "nope"}));
        assert!(result["error"].as_str().unwrap().contains("Search"));
    }

    #[test]
    fn a_name_match_beats_a_description_match() {
        let named = entry("create_issue", "does tracker work");
        let described = entry("zeta", "create an issue in the tracker");
        let query = words("create issue");
        assert!(score(&query, &named) > score(&query, &described));
    }

    #[test]
    fn a_search_describes_the_matching_tool_and_names_the_rest() {
        let registry =
            registry_with(&[("create_issue", "Open a tracker issue"), ("deploy", "Ship")]);
        let catalogue = Catalogue {
            registry: Some(&registry),
            live: None,
            withheld: Withheld::default(),
        };
        let result = search(&catalogue, &json!({"query": "open an issue"}));
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools[0]["name"], "create_issue");
        // A query result carries the schema itself, not a shorthand of it:
        // the model sends that JSON back in the direct call.
        assert_eq!(
            tools[0]["parameters"]["properties"]["x"]["type"],
            json!("string")
        );
        // Only tools that matched are listed: an unrelated tool is not
        // offered in the "other names" tail, because nothing recommends it.
        let rest = result["other_names"].as_array().unwrap();
        assert!(!rest.iter().any(|name| name == "deploy"));
        assert_eq!(result["total"], 2);
    }

    /// A query phrased as work, not as a tool name, must still shortlist the
    /// tool that does it.
    ///
    /// Ranking is lexical, so this measures RECALL OF THE SHORTLIST - the three
    /// tools a search describes in full - and not semantic understanding. The
    /// point is to keep a query a reader would plausibly type from missing the
    /// obvious tool, because a miss costs a second search and possibly the turn.
    #[test]
    fn a_search_shortlists_the_tool_a_query_means() {
        let registry = registry_with(&[
            (
                "bash",
                "Run a shell command with sh -c and return its stdout, stderr and exit code.",
            ),
            (
                "read_file",
                "Read a UTF-8 text file. Paths may be absolute or relative to the workspace.",
            ),
            (
                "edit_file",
                "Replace an exact string in a UTF-8 text file inside the workspace.",
            ),
            (
                "write_file",
                "Write a UTF-8 text file, creating parent directories as needed.",
            ),
            ("glob", "Find files whose paths match a glob pattern."),
            (
                "grep",
                "Search code (file contents) with a ripgrep pattern, a regular expression.",
            ),
            (
                "update_plan",
                "Record and update a task list to plan multi-step work and show progress.",
            ),
            (
                "job_output",
                "Read a background job's captured output and status.",
            ),
            ("web_search", "Search the web and return ranked sources."),
            (
                "browser_open",
                "Open an http(s) URL in the built-in Browser pane.",
            ),
            (
                "github_create_issue",
                "Open a new issue in a GitHub repository.",
            ),
            (
                "github_search",
                "Search GitHub issues in the connected repository.",
            ),
        ]);
        let catalogue = Catalogue {
            registry: Some(&registry),
            live: None,
            withheld: Withheld::default(),
        };

        let cases = [
            ("open a github issue", "github_create_issue"),
            ("search github for issues", "github_search"),
            ("run a command", "bash"),
            ("read a file", "read_file"),
            ("replace text in a file", "edit_file"),
            ("find files by name", "glob"),
            ("search the code for a pattern", "grep"),
            ("check on a background job", "job_output"),
            ("browse a web page", "browser_open"),
            ("plan the steps", "update_plan"),
        ];

        let mut misses = Vec::new();
        for (query, expected) in cases {
            let result = search(&catalogue, &json!({ "query": query }));
            let shortlist: Vec<&str> = result["tools"]
                .as_array()
                .expect("a shortlist")
                .iter()
                .filter_map(|tool| tool["name"].as_str())
                .collect();
            if !shortlist.contains(&expected) {
                misses.push(format!("{query:?} -> {shortlist:?}, wanted {expected}"));
            }
        }
        assert!(
            misses.is_empty(),
            "every query must shortlist its tool:\n{}",
            misses.join("\n")
        );
    }

    /// The shortlist is for tools the model does NOT already have: a direct
    /// tool is not a search result, and naming one says so instead of handing
    /// back a definition the model can already see.
    #[test]
    fn a_tool_the_model_already_has_is_not_offered() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(FunctionTool::new(
                "read_file",
                "Read a UTF-8 text file from the workspace.",
                json!({"type": "object", "properties": {}}),
                |_| Ok(json!({})),
            )))
            .unwrap();
        registry
            .register(Arc::new(FunctionTool::new(
                "github_search",
                "Search GitHub issues in the connected repository.",
                json!({"type": "object", "properties": {}}),
                |_| Ok(json!({})),
            )))
            .unwrap();
        registry.mark_mcp_tool("github_search");
        let catalogue = Catalogue {
            registry: Some(&registry),
            live: None,
            withheld: Withheld::default(),
        };

        let result = search(
            &catalogue,
            &json!({"query": "read a file and search issues"}),
        );
        let names: Vec<&str> = result["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(names, vec!["github_search"]);
        assert_eq!(result["total"], 1, "only withheld tools are searchable");

        let exact = search(&catalogue, &json!({"name": "read_file"}));
        assert!(
            exact["error"]
                .as_str()
                .unwrap_or_default()
                .contains("already in your tool list"),
            "naming a direct tool says so: {exact}"
        );
    }

    #[test]
    fn a_search_without_words_lists_names() {
        let registry = registry_with(&[("alpha", "does alpha"), ("beta", "does beta")]);
        let catalogue = Catalogue {
            registry: Some(&registry),
            live: None,
            withheld: Withheld::default(),
        };
        let result = search(&catalogue, &json!({}));
        assert_eq!(result["names"].as_array().unwrap().len(), 2);
        assert_eq!(result["total"], 2);
    }

    #[test]
    fn an_exact_name_returns_its_full_schema() {
        let registry = registry_with(&[("alpha", "does alpha")]);
        let catalogue = Catalogue {
            registry: Some(&registry),
            live: None,
            withheld: Withheld::default(),
        };
        let result = search(&catalogue, &json!({"name": "alpha"}));
        assert_eq!(result["name"], "alpha");
        assert_eq!(result["parameters"]["type"], "object");
    }

    /// A skill or command names a tool the way its author says it, so a bare
    /// word has to reach the package's namespaced tool. Measured: a packaged
    /// skill said "call design", the registry held `extdev__design`, and the
    /// exact-match lookup spent a round trip answering "no tool named 'design'".
    #[test]
    fn a_bare_word_resolves_to_the_namespaced_tool_it_names() {
        let registry = registry_with(&[("extdev__design", "Design an extension")]);
        let catalogue = Catalogue {
            registry: Some(&registry),
            live: None,
            withheld: Withheld::default(),
        };
        let result = search(&catalogue, &json!({"name": "design"}));
        assert_eq!(
            result["name"], "extdev__design",
            "the answer carries the name the direct call can run: {result}"
        );
        assert!(result["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("extdev__design"));
    }

    /// Two tools whose last segment is the same word stay unresolved: picking
    /// one would run a tool the model did not ask for.
    #[test]
    fn an_ambiguous_bare_word_does_not_guess() {
        let registry = registry_with(&[("alpha__status", "one"), ("beta__status", "two")]);
        let catalogue = Catalogue {
            registry: Some(&registry),
            live: None,
            withheld: Withheld::default(),
        };
        let result = search(&catalogue, &json!({"name": "status"}));
        assert!(
            result["error"]
                .as_str()
                .unwrap_or_default()
                .contains("No tool"),
            "an ambiguous word is not resolved: {result}"
        );
        let near: Vec<&str> = result["near_names"]
            .as_array()
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert!(
            near.contains(&"alpha__status") && near.contains(&"beta__status"),
            "both candidates are named instead: {result}"
        );
    }

    /// A shortlisted tool comes back with the JSON Schema the call has to
    /// match, not a rendering of it: the model sends args field for field, and
    /// a shorthand drops the nesting, the required list and the defaults.
    #[test]
    fn a_search_returns_the_argument_schema_a_call_has_to_match() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(FunctionTool::new(
                "create_issue",
                "Open a tracker issue",
                json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Issue title"},
                        "labels": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["title"]
                }),
                |_| Ok(json!({})),
            )))
            .unwrap();
        registry.mark_mcp_tool("create_issue");
        let catalogue = Catalogue {
            registry: Some(&registry),
            live: None,
            withheld: Withheld::default(),
        };

        let result = search(&catalogue, &json!({"query": "open an issue"}));
        let parameters = &result["tools"][0]["parameters"];
        assert_eq!(parameters["required"], json!(["title"]));
        assert_eq!(
            parameters["properties"]["title"]["description"],
            json!("Issue title")
        );
        assert!(
            result["tools"][0].get("schema").is_none(),
            "the free-form rendering is gone: {result}"
        );
    }
}
