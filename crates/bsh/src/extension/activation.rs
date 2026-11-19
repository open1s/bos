//! Task-scoped activation of extension prompt contributions (issue #436).
//!
//! An installed extension contributes instructions and workspace contexts. Its
//! presence in the workspace does not mean every turn needs its workflow: the
//! captured greeting carried the full Extension DevKit guide. This module
//! selects a contribution only when the current task, an explicit extension
//! command or an admitted extension capability makes it relevant, and reports
//! every other contribution with a reason.
//!
//! Activation has two levels:
//!
//! - an EXTENSION-level signal (an explicit command, an admitted capability the
//!   task names, or a task that names the extension) selects all of that
//!   extension's contributions;
//! - otherwise each CONTRIBUTION is evaluated on its own declared description,
//!   triggers, commands and tools. A later contribution can match while the
//!   first does not.
//!
//! Relevance is lexical and derived from the extension's own declared
//! metadata; nothing here knows what a particular ecosystem's vocabulary looks
//! like, so no domain word list has to be maintained. The tokenizer is the
//! memory-retrieval one, so a Chinese task and English metadata can meet on a
//! shared CJK bigram.
//!
//! Selection is deterministic and bounded:
//!
//! - groups and contributions are visited in owner/id order, so the request
//!   prefix cannot depend on registration order;
//! - a contribution whose body is empty (denied, empty, over the install
//!   budget) is omitted with its recorded diagnostic;
//! - a per-extension and an aggregate byte ceiling bound one turn; anything
//!   over either is omitted with an explicit reason, never silently dropped.
//!
//! Extension text is untrusted data. It cannot grant a capability or weaken a
//! permission: the policy engine authorizes invocation independently, and the
//! host's own security invariants are not extension contributions.

use std::collections::{BTreeMap, BTreeSet};

use agent::agent::ConditionalPrompt;

/// Most bytes one extension may activate in a single turn.
pub(crate) const MAX_ACTIVE_PER_EXTENSION: usize = 8 * 1024;

/// Most bytes all extensions may activate in a single turn.
pub(crate) const MAX_ACTIVE_TOTAL: usize = 24 * 1024;

/// Small, deterministic bridge for the task verbs and nouns a Chinese reader
/// uses with English extension metadata. This is not a keyword framework: it
/// maps one CJK term to the canonical English word the metadata already
/// carries, and every other term still matches through the shared tokenizer.
const CJK_ALIASES: &[(&str, &str)] = &[
    ("构建", "build"),
    ("创建", "build"),
    ("开发", "develop"),
    ("设计", "design"),
    ("验证", "validate"),
    ("校验", "validate"),
    ("发布", "publish"),
    ("扩展", "extension"),
    ("插件", "extension"),
    ("修复", "fix"),
    ("测试", "test"),
    ("检查", "check"),
    ("文档", "documentation"),
    ("配置", "config"),
    ("部署", "deploy"),
];

/// Language stop words, not domain vocabulary. They carry no topic on their
/// own, so they never make a contribution relevant.
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "that", "this", "these", "those", "your", "you", "how",
    "when", "where", "what", "why", "who", "into", "are", "was", "were", "will", "can", "could",
    "should", "must", "may", "might", "have", "has", "had", "use", "using", "used", "one", "two",
    "all", "any", "its", "their", "our", "out", "not", "but", "also", "via", "per", "each", "more",
    "most", "some", "such", "than", "then",
];

/// One contribution selected for this turn, with its own reason.
#[derive(Debug, Clone)]
pub(crate) struct ActivePrompt {
    /// The contribution.
    pub(crate) prompt: ConditionalPrompt,
    /// Why this contribution is active.
    pub(crate) reason: String,
}

/// One extension selected for this turn.
#[derive(Debug, Clone)]
pub(crate) struct ActiveExtension {
    /// Extension id.
    pub(crate) owner: String,
    /// Display name.
    pub(crate) name: String,
    /// Active package version.
    pub(crate) version: String,
    /// Trust level name.
    pub(crate) trust: String,
    /// Extension-level reason, when the whole extension was selected.
    pub(crate) reason: String,
    /// Included contributions, each with its own reason.
    pub(crate) prompts: Vec<ActivePrompt>,
    /// Bytes these contributions cost.
    pub(crate) bytes: usize,
}

/// One contribution this turn does not carry, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtensionOmission {
    /// Extension id.
    pub(crate) owner: String,
    /// Active package version.
    pub(crate) version: String,
    /// Trust level name.
    pub(crate) trust: String,
    /// Contribution id.
    pub(crate) contribution: String,
    /// Contribution kind.
    pub(crate) kind: String,
    /// Why it is omitted.
    pub(crate) reason: String,
    /// Bytes it would have cost.
    pub(crate) bytes: usize,
}

/// What this turn activates.
#[derive(Debug, Clone, Default)]
pub(crate) struct ActivationPlan {
    /// Selected extensions and their contributions, in owner order.
    pub(crate) active: Vec<ActiveExtension>,
    /// Omitted contributions with a reason.
    pub(crate) omissions: Vec<ExtensionOmission>,
}

impl ActivationPlan {
    /// Bytes the active contributions cost.
    pub(crate) fn bytes(&self) -> usize {
        self.active.iter().map(|extension| extension.bytes).sum()
    }

    /// Whether anything is active.
    pub(crate) fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
}

/// Select the extension contributions one turn needs.
///
/// `admitted_tools` is the authoritative capability view, not the initial
/// model-visible array: a deferred extension capability is absent from the
/// array but still admitted, and naming it must activate its guidance.
pub(crate) fn activate(
    prompts: &[ConditionalPrompt],
    task: &str,
    admitted_tools: &[String],
) -> ActivationPlan {
    let task_tokens = tokenize(task);
    let admitted: BTreeSet<&str> = admitted_tools.iter().map(String::as_str).collect();
    let mut groups: BTreeMap<&str, Vec<&ConditionalPrompt>> = BTreeMap::new();
    for prompt in prompts {
        groups
            .entry(prompt.owner.as_str())
            .or_default()
            .push(prompt);
    }

    let mut plan = ActivationPlan::default();
    let mut total = 0usize;
    for (_owner, mut contributions) in groups {
        contributions.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.kind.cmp(&right.kind))
        });
        let first = contributions[0];
        let extension_reason =
            extension_signal(first, &contributions, &task_tokens, task, &admitted);
        let mut extension = ActiveExtension {
            owner: first.owner.clone(),
            name: first.owner_name.clone(),
            version: first.version.clone(),
            trust: first.trust.clone(),
            reason: extension_reason.clone().unwrap_or_default(),
            prompts: Vec::new(),
            bytes: 0,
        };
        for prompt in contributions {
            let reason = extension_reason
                .clone()
                .or_else(|| contribution_signal(prompt, &task_tokens, task, &admitted));
            let Some(reason) = reason else {
                plan.omissions.push(omission(
                    prompt,
                    if prompt.text.is_empty() {
                        prompt
                            .diagnostic
                            .clone()
                            .unwrap_or_else(|| "empty contribution".to_string())
                    } else {
                        "not relevant to this contribution".to_string()
                    },
                ));
                continue;
            };
            if prompt.text.is_empty() {
                plan.omissions.push(omission(
                    prompt,
                    prompt
                        .diagnostic
                        .clone()
                        .unwrap_or_else(|| "empty contribution".to_string()),
                ));
                continue;
            }
            let bytes = prompt.text.len();
            if extension.bytes.saturating_add(bytes) > MAX_ACTIVE_PER_EXTENSION {
                plan.omissions.push(omission(
                    prompt,
                    format!(
                        "over the per-extension turn budget ({MAX_ACTIVE_PER_EXTENSION} bytes)"
                    ),
                ));
                continue;
            }
            if total.saturating_add(bytes) > MAX_ACTIVE_TOTAL {
                plan.omissions.push(omission(
                    prompt,
                    format!("over the aggregate turn budget ({MAX_ACTIVE_TOTAL} bytes)"),
                ));
                continue;
            }
            total = total.saturating_add(bytes);
            extension.bytes = extension.bytes.saturating_add(bytes);
            extension.prompts.push(ActivePrompt {
                prompt: prompt.clone(),
                reason,
            });
        }
        if !extension.prompts.is_empty() {
            plan.active.push(extension);
        }
    }
    plan
}

/// An extension-level signal: the whole extension is relevant.
fn extension_signal(
    first: &ConditionalPrompt,
    contributions: &[&ConditionalPrompt],
    task_tokens: &BTreeSet<String>,
    task: &str,
    admitted: &BTreeSet<&str>,
) -> Option<String> {
    // An admitted capability is relevant only when THIS task names it. Merely
    // having an extension tool installed must not inject its guide into an
    // unrelated work turn (issue #436 review): a research question about LLMs
    // must not carry the extension-development workflow.
    if let Some(command) = first
        .commands
        .iter()
        .find(|command| task.contains(&format!("name=\"{command}\"")))
    {
        return Some(format!("explicit command /{command}"));
    }
    for prompt in contributions {
        if let Some(tool) = named_capability(prompt, task_tokens, task, admitted) {
            return Some(format!("task names admitted capability {tool}"));
        }
    }
    let lower = task.to_lowercase();
    if lower.contains(&first.owner.to_lowercase())
        || lower.contains(&first.owner_name.to_lowercase())
    {
        return Some("task names the extension".to_string());
    }
    None
}

/// A contribution-level signal.
fn contribution_signal(
    prompt: &ConditionalPrompt,
    task_tokens: &BTreeSet<String>,
    task: &str,
    admitted: &BTreeSet<&str>,
) -> Option<String> {
    if let Some(tool) = named_capability(prompt, task_tokens, task, admitted) {
        return Some(format!("task names admitted capability {tool}"));
    }
    let mut metadata = BTreeSet::new();
    for trigger in &prompt.triggers {
        metadata.extend(tokenize(trigger));
    }
    for command in &prompt.commands {
        metadata.extend(tokenize(command));
    }
    for tool in &prompt.tools {
        metadata.extend(tokenize(tool));
    }
    let hits: Vec<&String> = metadata.intersection(task_tokens).collect();
    if hits.len() >= 2 {
        return Some(format!(
            "task shares {} terms with this contribution",
            hits.len()
        ));
    }
    // A single shared term is too weak: "fix the failing build" shares "build"
    // with an extension guide but asks nothing about that extension.
    None
}

/// The contribution's tool that the task names AND the capability view admits.
fn named_capability(
    prompt: &ConditionalPrompt,
    task_tokens: &BTreeSet<String>,
    task: &str,
    admitted: &BTreeSet<&str>,
) -> Option<String> {
    let lower_task = task.to_lowercase();
    prompt.tools.iter().find_map(|tool| {
        let admitted_tool = admitted.contains(tool.as_str());
        if !admitted_tool {
            return None;
        }
        let lower = tool.to_lowercase();
        let named = if lower.contains("__") {
            !lower_task.is_empty() && lower_task.contains(&lower)
        } else {
            tokenize(tool)
                .into_iter()
                .next()
                .is_some_and(|term| task_tokens.contains(&term))
        };
        named.then(|| tool.clone())
    })
}

/// The extension's own identity terms: its namespace and display name.
fn identity_tokens(prompt: &ConditionalPrompt) -> BTreeSet<String> {
    let namespace = prompt
        .owner
        .rsplit('.')
        .next()
        .unwrap_or(prompt.owner.as_str());
    let mut terms = tokenize(namespace);
    terms.extend(tokenize(&prompt.owner_name));
    terms
}

/// Normalize text to lowercase content terms.
fn tokenize(text: &str) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    for token in agent::memory::retrieval::tokenize(text) {
        let token = token.strip_suffix('s').unwrap_or(&token).to_string();
        let width = token.chars().count();
        let cjk = !token.is_ascii();
        let keep = if cjk { width >= 2 } else { width >= 3 };
        if !keep || STOP_WORDS.contains(&token.as_str()) {
            continue;
        }
        for (cjk_term, english) in CJK_ALIASES {
            if token == *cjk_term {
                terms.insert((*english).to_string());
            }
        }
        terms.insert(token);
    }
    terms
}

fn omission(prompt: &ConditionalPrompt, reason: String) -> ExtensionOmission {
    ExtensionOmission {
        owner: prompt.owner.clone(),
        version: prompt.version.clone(),
        trust: prompt.trust.clone(),
        contribution: prompt.id.clone(),
        kind: prompt.kind.clone(),
        reason,
        bytes: prompt.text.len(),
    }
}

/// Render one active contribution in the provider's block shape.
pub(crate) fn render(active: &ActiveExtension, selected: &ActivePrompt) -> String {
    format!(
        "## Extension {}: {} ({}@{})
Activation: {}
{}",
        selected.prompt.kind,
        selected.prompt.description,
        active.owner,
        active.version,
        selected.reason,
        selected.prompt.text
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(owner: &str, name: &str, id: &str, text: &str) -> ConditionalPrompt {
        ConditionalPrompt {
            owner: owner.to_string(),
            owner_name: name.to_string(),
            version: "1.2.0".to_string(),
            trust: "community".to_string(),
            id: id.to_string(),
            kind: "prompt".to_string(),
            description: format!("{name} workflow"),
            text: text.to_string(),
            scope: Vec::new(),
            triggers: {
                let mut terms = tokenize(owner);
                terms.extend(tokenize(name));
                for verb in ["design", "build", "validate", "publish"] {
                    terms.insert(verb.to_string());
                }
                terms.into_iter().collect()
            },
            commands: vec![
                "devkit__scaffold".to_string(),
                "devkit__validate".to_string(),
            ],
            tools: vec![
                "devkit__scaffold".to_string(),
                "scaffold".to_string(),
                "validate".to_string(),
            ],
            tool_namespace: owner.rsplit('.').next().unwrap().to_string(),
            diagnostic: None,
        }
    }

    fn devkit() -> ConditionalPrompt {
        prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            "How to build extensions",
        )
    }

    #[test]
    fn a_greeting_activates_nothing() {
        let plan = activate(&[devkit()], "hello 8888", &[]);
        assert!(plan.is_empty());
        assert_eq!(plan.omissions.len(), 1);
        assert_eq!(
            plan.omissions[0].reason,
            "not relevant to this contribution"
        );
    }

    #[test]
    fn extension_work_activates_the_guide() {
        let plan = activate(&[devkit()], "build a new extension package", &[]);
        assert_eq!(plan.active.len(), 1);
        assert!(plan.active[0].reason.is_empty());
        assert!(plan.active[0].prompts[0].reason.contains("shares 2 terms"));
    }

    #[test]
    fn metadata_overlap_activates_without_an_action_verb() {
        let plan = activate(&[devkit()], "validate my extension", &[]);
        assert_eq!(plan.active.len(), 1, "{plan:?}");
        assert!(plan.active[0].prompts[0].reason.contains("shares"));
    }

    #[test]
    fn an_explicit_command_activates_the_whole_extension() {
        let plan = activate(
            &[devkit()],
            "<extension_command name=\"devkit__scaffold\">\nScaffold.\n</extension_command>",
            &[],
        );
        assert_eq!(plan.active.len(), 1);
        assert_eq!(plan.active[0].reason, "explicit command /devkit__scaffold");
        assert!(
            plan.active[0].prompts[0]
                .reason
                .contains("explicit command")
        );
    }

    #[test]
    fn a_named_admitted_capability_activates_even_when_withheld() {
        let plan = activate(
            &[devkit()],
            "run devkit__scaffold for this package",
            &["devkit__scaffold".to_string()],
        );
        assert_eq!(plan.active.len(), 1, "{plan:?}");
        assert!(
            plan.active[0].prompts[0]
                .reason
                .contains("admitted capability")
        );
    }

    #[test]
    fn an_admitted_but_unnamed_capability_does_not_activate() {
        let plan = activate(&[devkit()], "hello 8888", &["devkit__scaffold".to_string()]);
        assert!(plan.is_empty());
    }

    #[test]
    fn an_admitted_deferred_capability_activates_when_the_task_names_it() {
        let plan = activate(
            &[devkit()],
            "run devkit__scaffold for this build",
            &["devkit__scaffold".to_string()],
        );
        assert_eq!(plan.active.len(), 1, "{plan:?}");
        assert!(
            plan.active[0]
                .reason
                .contains("task names admitted capability")
        );
    }

    /// The reported regression: an admitted extension tool does not make a
    /// research turn about LLMs carry the extension workflow.
    #[test]
    fn a_research_turn_does_not_activate_an_unrelated_extension() {
        let plan = activate(
            &[devkit()],
            "research the latest llm papers",
            &["devkit__scaffold".to_string()],
        );
        assert!(plan.is_empty(), "{plan:?}");
    }

    #[test]
    fn an_admitted_capability_does_not_activate_conversation() {
        let plan = activate(&[devkit()], "hello 8888", &["devkit__scaffold".to_string()]);
        assert!(plan.is_empty());
    }

    #[test]
    fn a_plain_extension_question_is_not_extension_work() {
        let plan = activate(&[devkit()], "what is a browser extension?", &[]);
        assert!(plan.is_empty());
    }

    #[test]
    fn an_unrelated_work_turn_does_not_activate_every_extension() {
        let plan = activate(&[devkit()], "fix the failing build", &[]);
        assert!(plan.is_empty(), "one shared word is not a topic: {plan:?}");
    }

    #[test]
    fn a_chinese_task_activates_english_metadata() {
        let mut zh = prompt(
            "com.example.zh",
            "扩展开发套件",
            "guide",
            "扩展开发套件工作流",
        );
        // The declared metadata is English; the task is Chinese. Only a common
        // semantic term can connect them.
        zh.triggers = vec!["extension".to_string(), "build".to_string()];
        zh.owner_name = "Extension Toolkit".to_string();
        zh.commands.clear();
        zh.tools.clear();
        let plan = activate(&[zh], "帮我构建一个扩展", &[]);
        assert_eq!(plan.active.len(), 1, "{plan:?}");
        assert!(plan.active[0].prompts[0].reason.contains("shares"));
    }

    #[test]
    fn a_chinese_greeting_does_not_activate() {
        let plan = activate(&[devkit()], "你好", &[]);
        assert!(plan.is_empty());
    }

    #[test]
    fn a_later_contribution_can_match_on_its_own() {
        let mut first = prompt(
            "com.example.tool",
            "Toolkit",
            "build-guide",
            "Build workflow",
        );
        first.triggers = vec!["build".to_string()];
        let mut second = prompt(
            "com.example.tool",
            "Toolkit",
            "checklist",
            "Validation checklist",
        );
        second.triggers = vec!["checklist".to_string(), "validate".to_string()];
        let plan = activate(&[first, second], "validate the checklist", &[]);
        assert_eq!(plan.active.len(), 1, "{plan:?}");
        let selected: Vec<&str> = plan.active[0]
            .prompts
            .iter()
            .map(|selected| selected.prompt.id.as_str())
            .collect();
        assert_eq!(selected, vec!["checklist"]);
        assert!(
            plan.omissions
                .iter()
                .any(|omission| omission.contribution == "build-guide"
                    && omission.reason == "not relevant to this contribution"),
            "the first contribution is reported: {:?}",
            plan.omissions
        );
    }

    #[test]
    fn only_the_relevant_extension_activates() {
        let devkit = prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            "How to build extensions",
        );
        let git = prompt("com.example.git", "Git Helper", "guide", "Git workflow");
        let plan = activate(&[git, devkit], "build a new extension package", &[]);
        assert_eq!(plan.active.len(), 1);
        assert_eq!(plan.active[0].owner, "com.example.devkit");
        assert!(
            plan.omissions
                .iter()
                .any(|omission| omission.owner == "com.example.git"
                    && omission.reason == "not relevant to this contribution"),
            "the unrelated extension is reported: {:?}",
            plan.omissions
        );
    }

    #[test]
    fn every_contribution_of_an_active_extension_is_included() {
        let guide = prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            "How to build extensions",
        );
        let checklist = prompt(
            "com.example.devkit",
            "Extension DevKit",
            "checklist",
            "Validation checklist",
        );
        let plan = activate(&[guide, checklist], "build a new extension package", &[]);
        assert_eq!(plan.active.len(), 1);
        assert_eq!(plan.active[0].prompts.len(), 2);
        assert!(
            plan.active[0]
                .prompts
                .iter()
                .all(|selected| selected.reason.contains("shares"))
        );
    }

    #[test]
    fn budgets_omit_with_a_reason() {
        let plan = activate(
            &[prompt(
                "com.example.devkit",
                "Extension DevKit",
                "guide",
                &"x".repeat(MAX_ACTIVE_PER_EXTENSION + 1),
            )],
            "build an extension",
            &[],
        );
        assert!(plan.is_empty());
        assert!(
            plan.omissions[0]
                .reason
                .contains("per-extension turn budget")
        );
    }

    #[test]
    fn the_aggregate_budget_omits_later_extensions() {
        let boxed = |owner: &str, name: &str| prompt(owner, name, "guide", &"x".repeat(7 * 1024));
        let prompts = vec![
            boxed("com.example.one", "Extension One"),
            boxed("com.example.two", "Extension Two"),
            boxed("com.example.three", "Extension Three"),
            boxed("com.example.four", "Extension Four"),
        ];
        let plan = activate(&prompts, "build an extension", &[]);
        assert!(plan.bytes() <= MAX_ACTIVE_TOTAL);
        assert!(
            plan.omissions
                .iter()
                .any(|omission| omission.reason.contains("aggregate turn budget"))
        );
    }

    #[test]
    fn denied_bodies_are_omitted_with_their_diagnostic() {
        let mut denied = devkit();
        denied.text.clear();
        denied.diagnostic = Some("capability agent.prompts.provide denied by policy".to_string());
        let plan = activate(&[denied], "build an extension", &[]);
        assert!(plan.is_empty());
        assert!(plan.omissions[0].reason.contains("denied by policy"));
    }
}
