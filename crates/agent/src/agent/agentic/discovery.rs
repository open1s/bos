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

/// The discovery section for the tool array a request actually carries.
///
/// The array is the one fact that says what the model may call. A prompt that
/// named a search the request does not carry tells the model to call a tool it
/// was never given, and a prompt that omitted one the request does carry hides
/// the way back to every withheld tool - so the block is derived from the same
/// list the request sends, never from a second guess at the same policy.
///
/// The heading is written once, and a paragraph appears only for the search
/// that is really there. An empty return means this request carries nothing to
/// search, and the model is told nothing it cannot do.
pub(crate) fn prompt_block(tools: &[LlmTool]) -> String {
    let tool_search = tools.iter().any(|tool| tool.name == FIND_TOOL);
    let skill_search = tools.iter().any(|tool| tool.name == FIND_SKILL);
    if !tool_search && !skill_search {
        return String::new();
    }
    let mut block = String::from(crate::agent::prompts::DISCOVERY_PRIORITY);
    // The heading names the searches it introduces, and only those. Naming
    // both unconditionally was the first draft of this block, and it told a
    // request that carried only one search to call the other.
    block.push_str(&match (tool_search, skill_search) {
        (true, true) => format!(" with {FIND_TOOL} or {FIND_SKILL}"),
        (true, false) => format!(" with {FIND_TOOL}"),
        (false, true) => format!(" with {FIND_SKILL}"),
        (false, false) => String::new(),
    });
    if tool_search {
        block.push_str("\n\n");
        block.push_str(crate::agent::prompts::DISCOVERY_GUIDANCE);
    }
    if skill_search {
        block.push_str("\n\n");
        block.push_str(crate::agent::prompts::SKILL_DISCOVERY_GUIDANCE);
    }
    block
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
    metadata: crate::skills::SkillMetadata,
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
            metadata: skill.metadata.clone(),
            dir: skill.skill_dir.to_string_lossy().to_string(),
        });
    }
    entries.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
    entries.dedup_by(|a, b| a.metadata.name == b.metadata.name);
    entries
}

/// One skill search: an exact name, a ranked query, or every name.
fn search_skills(entries: &[SkillEntry], args: &Value) -> Value {
    if let Some(name) = args.get("name").and_then(Value::as_str) {
        return match entries.iter().find(|entry| entry.metadata.name == name) {
            Some(entry) => json!({
                "name": entry.metadata.name,
                "description": entry.metadata.description,
                "version": entry.metadata.version.display(),
                "category": entry.metadata.category.as_str(),
                "tags": entry.metadata.tags,
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
            .map(|entry| entry.metadata.name.as_str())
            .take(FIND_SKILL_NAMES)
            .collect();
        return json!({
            "total": entries.len(),
            "names": names,
            "hint": "Call find_skill with a query or a name, then load_skill to read one.",
        });
    }

    // Ranking uses the Skill metadata-aware scorer, which weighs category
    // and tags and normalizes Unicode; it returns a deterministic name
    // tiebreak. Results stay metadata-only.
    let mut ranked: Vec<(i64, &SkillEntry)> = entries
        .iter()
        .map(|entry| {
            (
                crate::skills::index::score_metadata(query, &entry.metadata),
                entry,
            )
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.metadata.name.cmp(&b.1.metadata.name))
    });

    let mut described = Vec::new();
    let mut spent = 0usize;
    for (_, entry) in ranked.iter().take(FIND_SKILL_DESCRIBED) {
        spent += entry.metadata.name.len() + entry.metadata.description.len();
        if spent > FIND_SKILL_BUDGET && !described.is_empty() {
            break;
        }
        described.push(json!({
            "name": entry.metadata.name,
            "description": clip(&entry.metadata.description, FIND_DESCRIPTION_CHARS),
            "version": entry.metadata.version.display(),
            "category": entry.metadata.category.as_str(),
            "tags": entry.metadata.tags,
        }));
    }
    let described_names: Vec<String> = described
        .iter()
        .filter_map(|value| value.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let names_only: Vec<&str> = ranked
        .iter()
        .map(|(_, entry)| entry.metadata.name.as_str())
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

    fn entries(&self) -> Vec<ToolEntry> {
        let mut out: Vec<ToolEntry> = Vec::new();
        for registry in [self.registry, self.live].into_iter().flatten() {
            for (name, tool) in registry.iter() {
                if !self.is_withheld(name, tool.deferred(), registry.is_mcp_tool(name)) {
                    continue;
                }
                out.push(ToolEntry {
                    name: name.clone(),
                    description: tool.description(),
                    schema: tool.json_schema(),
                    metadata: tool.metadata().cloned().unwrap_or_default(),
                });
            }
            for name in registry.async_tool_names() {
                if let Some(tool) = registry.get_async(&name) {
                    if !self.is_withheld(&name, tool.deferred(), registry.is_mcp_tool(&name)) {
                        continue;
                    }
                    out.push(ToolEntry {
                        name: name.clone(),
                        description: tool.description(),
                        schema: tool.json_schema(),
                        metadata: tool.metadata().cloned().unwrap_or_default(),
                    });
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out.dedup_by(|a, b| a.name == b.name);
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
    view: Arc<RwLock<Option<Arc<ToolRegistry>>>>,
    withheld: Withheld,
) -> FunctionTool {
    FunctionTool::new(
        FIND_TOOL,
        FIND_DESCRIPTION,
        find_tool_schema(),
        move |args: &Value| {
            let guard = live.read().ok();
            // The authoritative registry is the one the agent currently runs,
            // not the one captured when the engine was built. A host that
            // replaces the registry through the capability composition (the
            // workspace view) publishes it here, so a search cannot offer a
            // tool the view no longer admits.
            let current = view.read().ok().and_then(|view| view.clone());
            let base = current.as_deref().or(registry.as_deref());
            let catalogue = Catalogue {
                registry: base,
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
            .map(|entry| entry.name.as_str())
            .take(FIND_NAMES)
            .collect();
        return json!({
            "total": entries.len(),
            "names": names,
            "hint": "Call find_tool with a query or a name to get a tool's arguments; reported tools join your tool list.",
        });
    }

    let mut ranked: Vec<(i64, &ToolEntry)> = entries
        .iter()
        .map(|entry| (score(query, entry), entry))
        .filter(|(score, _)| *score > 0)
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));

    let mut described = Vec::new();
    let mut spent = 0usize;
    for (_, entry) in ranked.iter().take(limit) {
        // Price the schema itself, not a rendering of it: the JSON below is
        // what the model reads, and parameter definitions are the reason it
        // asked.
        spent += entry.name.len() + entry.description.len() + entry.schema.to_string().len();
        if spent > FIND_BUDGET && !described.is_empty() {
            break;
        }
        described.push(json!({
            "name": entry.name,
            "description": clip(&entry.description, FIND_DESCRIPTION_CHARS),
            "parameters": entry.schema,
            // A declared version is the schema's revision, so a changed
            // definition is priced again rather than reusing a stale one.
            "version": entry.metadata.version,
            // The model keeps calling the surface name; these carry the
            // authoritative runtime identity alongside it.
            "capability_id": entry.metadata.capability_id,
            "capability_version": entry.metadata.capability_version,
            "provider": entry.metadata.provider,
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

/// One withheld tool as discovery sees it.
struct ToolEntry {
    name: String,
    description: String,
    schema: Value,
    metadata: react::tool::ToolMetadata,
}

/// How well one tool answers the query.
///
/// A name match outranks metadata, which outranks a description mention. The
/// weights are the documented contract: exact name 120, name prefix 60, name
/// term 25, tag 18, intent 18, category 12, description 6.
fn score(query: &str, entry: &ToolEntry) -> i64 {
    let normalized = query.trim().to_lowercase();
    if normalized.is_empty() {
        return 0;
    }
    let name = entry.name.to_lowercase();
    let description = entry.description.to_lowercase();
    let category = entry
        .metadata
        .category
        .as_deref()
        .unwrap_or("")
        .to_lowercase();
    let tags: Vec<String> = entry
        .metadata
        .tags
        .iter()
        .map(|tag| tag.to_lowercase())
        .collect();
    let intents: Vec<String> = entry
        .metadata
        .intents
        .iter()
        .map(|intent| intent.to_lowercase())
        .collect();
    let mut total = 0i64;
    if name == normalized {
        total += 120;
    } else if name.starts_with(&normalized) {
        total += 60;
    }
    for term in crate::skills::index::terms(query) {
        if name.contains(&term) {
            total += 25;
        }
        if tags.iter().any(|tag| tag.contains(&term)) {
            total += 18;
        }
        if intents.iter().any(|intent| intent.contains(&term)) {
            total += 18;
        }
        if category.contains(&term) {
            total += 12;
        }
        if description.contains(&term) {
            total += 6;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, description: &str) -> ToolEntry {
        entry_with_metadata(name, description, react::tool::ToolMetadata::default())
    }

    fn entry_with_metadata(
        name: &str,
        description: &str,
        metadata: react::tool::ToolMetadata,
    ) -> ToolEntry {
        ToolEntry {
            name: name.to_string(),
            description: description.to_string(),
            schema: json!({"type": "object", "properties": {"x": {"type": "string"}}}),
            metadata,
        }
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

    /// The discovery block follows the REQUEST's tool array: a search the
    /// request carries is named, one it does not carry is silent, and the
    /// heading is written once even when both searches ride the same request.
    #[test]
    fn the_discovery_block_names_exactly_the_searches_the_request_carries() {
        let tool = |name: &str| LlmTool::function(name, "does a thing", json!({"type": "object"}));

        assert!(
            prompt_block(&[]).is_empty(),
            "an empty array has nothing to search"
        );

        let tools = prompt_block(&[tool(FIND_TOOL)]);
        assert!(tools.starts_with(crate::agent::prompts::DISCOVERY_PRIORITY));
        assert!(
            tools.contains(&format!("with {FIND_TOOL}")),
            "the heading names the search it introduces: {tools}"
        );
        assert!(
            !tools.contains(FIND_SKILL),
            "a search the request does not carry is not named"
        );
        assert!(tools.contains(crate::agent::prompts::DISCOVERY_GUIDANCE));
        assert!(!tools.contains(crate::agent::prompts::SKILL_DISCOVERY_GUIDANCE));
        assert_eq!(
            tools
                .matches(crate::agent::prompts::DISCOVERY_PRIORITY)
                .count(),
            1,
            "one heading, however many paragraphs follow it"
        );

        let skills = prompt_block(&[tool(FIND_SKILL)]);
        assert!(skills.starts_with(crate::agent::prompts::DISCOVERY_PRIORITY));
        assert!(skills.contains(&format!("with {FIND_SKILL}")));
        assert!(!skills.contains(FIND_TOOL));
        assert!(skills.contains(crate::agent::prompts::SKILL_DISCOVERY_GUIDANCE));
        assert!(!skills.contains(crate::agent::prompts::DISCOVERY_GUIDANCE));

        let both = prompt_block(&[tool(FIND_TOOL), tool(FIND_SKILL)]);
        assert!(both.contains(&format!("with {FIND_TOOL} or {FIND_SKILL}")));
        assert!(both.contains(crate::agent::prompts::DISCOVERY_GUIDANCE));
        assert!(both.contains(crate::agent::prompts::SKILL_DISCOVERY_GUIDANCE));
        assert_eq!(
            both.matches(crate::agent::prompts::DISCOVERY_PRIORITY)
                .count(),
            1
        );
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
    fn a_skill_search_uses_tags_and_category_metadata() {
        let mut tagged = skill("release", "Publish a build", false);
        tagged.metadata.tags = vec!["deploy".to_string()];
        let mut categorised = skill("ops", "Run a job", false);
        categorised.metadata.category = crate::skills::SkillCategory::Data;
        let entries = skill_entries(&[tagged, categorised], None);

        let tagged_results = search_skills(&entries, &json!({"query": "deploy"}));
        assert_eq!(tagged_results["skills"][0]["name"], "release");
        assert_eq!(tagged_results["skills"][0]["tags"][0], "deploy");
        assert_eq!(tagged_results["skills"][0]["version"], "1.0.0");

        let category_results = search_skills(&entries, &json!({"query": "data"}));
        assert_eq!(category_results["skills"][0]["name"], "ops");
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
        assert!(score("create issue", &named) > score("create issue", &described));
    }

    #[test]
    fn tool_metadata_can_find_a_tool_whose_description_uses_other_words() {
        let tagged = entry_with_metadata(
            "deploy",
            "Publish a build",
            react::tool::ToolMetadata::default()
                .with_tags(["ship", "release"])
                .with_category("operations"),
        );
        let unrelated = entry("cooking", "Bake bread");
        assert!(score("ship the release", &tagged) > 0);
        assert_eq!(score("ship the release", &unrelated), 0);
        assert!(score("operations", &tagged) > 0, "category is searchable");
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

    /// A measured baseline for tool retrieval. Assertions pin recall and rank
    /// so a scoring change that loses a synonym fails the suite.
    #[test]
    fn the_tool_retrieval_corpus_reports_precision_recall_and_mrr() {
        let registry = registry_with(&[
            ("git_commit", "Create a git commit"),
            ("git_push", "Push commits to a remote"),
            ("deploy", "Ship a build"),
            ("read_file", "Read a file from the workspace"),
            ("notify", "Send a notification"),
            ("query_db", "Run a SQL query"),
        ]);
        let catalogue = Catalogue {
            registry: Some(&registry),
            live: None,
            withheld: Withheld::default(),
        };
        let entries = catalogue.entries();
        let cases: [(&str, &str); 5] = [
            ("commit changes", "git_commit"),
            ("push commits", "git_push"),
            ("ship a build", "deploy"),
            ("send a notification", "notify"),
            ("sql query", "query_db"),
        ];
        let mut precision = 0.0;
        let mut recall = 0.0;
        let mut mrr = 0.0;
        for (query, expected) in cases {
            let mut ranked: Vec<(i64, &str)> = entries
                .iter()
                .map(|entry| (score(query, entry), entry.name.as_str()))
                .filter(|(value, _)| *value > 0)
                .collect();
            ranked.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(right.1)));
            let names: Vec<String> = ranked
                .iter()
                .take(3)
                .map(|(_, name)| name.to_string())
                .collect();
            let relevant = std::collections::BTreeSet::from([expected.to_string()]);
            let metrics = crate::skills::runtime::retrieval_metrics(&names, &relevant, 3);
            eprintln!(
                "query={query:?} ranked={names:?} P@3={:.2} R@3={:.2} RR={:.2}",
                metrics.precision_at_k, metrics.recall_at_k, metrics.reciprocal_rank
            );
            assert!(metrics.recall_at_k > 0.0, "missed {expected} for {query:?}");
            precision += metrics.precision_at_k;
            recall += metrics.recall_at_k;
            mrr += metrics.reciprocal_rank;
        }
        let count = cases.len() as f64;
        eprintln!(
            "macro P@3={:.2} R@3={:.2} MRR={:.2}",
            precision / count,
            recall / count,
            mrr / count
        );
        assert!(precision / count >= 0.33, "precision regressed");
        assert!(recall / count >= 0.99, "recall regressed");
        assert!(mrr / count >= 0.8, "rank regressed");
    }

    #[test]
    fn a_capability_shim_reports_its_runtime_identity() {
        let metadata = react::tool::ToolMetadata::default()
            .with_capability("extdev/design", "2.1.0")
            .with_provider("builtin");
        let mut registry = ToolRegistry::new();
        let tool = FunctionTool::new(
            "design",
            "Design a service",
            json!({"type": "object", "properties": {"x": {"type": "string"}}}),
            |_| Ok(json!({"ok": true})),
        )
        .with_metadata(metadata);
        registry.register(Arc::new(tool)).expect("register");
        registry.mark_mcp_tool("design");
        let catalogue = Catalogue {
            registry: Some(&registry),
            live: None,
            withheld: Withheld::default(),
        };
        let result = search(&catalogue, &json!({"query": "design"}));
        assert_eq!(result["tools"][0]["capability_id"], "extdev/design");
        assert_eq!(result["tools"][0]["capability_version"], "2.1.0");
        assert_eq!(result["tools"][0]["provider"], "builtin");
    }

    #[test]
    fn discovery_reads_the_current_registry_view_not_the_build_snapshot() {
        use crate::tools::Tool as _;
        let built = Arc::new(registry_with(&[("stale", "a tool the view dropped")]));
        let current = Arc::new(registry_with(&[("fresh", "a tool the view admits")]));
        let view = Arc::new(RwLock::new(Some(current)));
        let find = find_tool(
            Some(built),
            Arc::new(RwLock::new(ToolRegistry::new())),
            view,
            Withheld::default(),
        );
        let listed = find.run(&json!({})).expect("the search runs");
        let names: Vec<&str> = listed["names"]
            .as_array()
            .expect("names")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(names.contains(&"fresh"), "{listed}");
        assert!(!names.contains(&"stale"), "{listed}");
    }

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
