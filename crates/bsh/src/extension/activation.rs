//! Task-scoped activation of extension prompt contributions (issue #436).
//!
//! An installed extension contributes instructions and workspace contexts. Its
//! presence in the workspace does not mean every turn needs its workflow: the
//! captured greeting carried the full Extension DevKit guide. This module
//! selects a contribution only when the current task, an explicit extension
//! command or an admitted extension capability makes it relevant, and reports
//! every other contribution with a reason.
//!
//! Relevance is lexical and derived from the extension's own declared
//! metadata - its id, display name, contribution descriptions, and command ids.
//! Nothing here knows what a particular ecosystem's vocabulary looks like, so
//! no domain word list has to be maintained.
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
use react::llm::ActionIntent;

/// Most bytes one extension may activate in a single turn.
pub(crate) const MAX_ACTIVE_PER_EXTENSION: usize = 8 * 1024;

/// Most bytes all extensions may activate in a single turn.
pub(crate) const MAX_ACTIVE_TOTAL: usize = 24 * 1024;

/// Language stop words, not domain vocabulary. They carry no topic on their
/// own, so they never make a contribution relevant.
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "that", "this", "these", "those", "your", "you", "how",
    "when", "where", "what", "why", "who", "into", "are", "was", "were", "will", "can", "could",
    "should", "must", "may", "might", "have", "has", "had", "use", "using", "used", "one", "two",
    "all", "any", "its", "their", "our", "out", "not", "but", "also", "via", "per", "each", "more",
    "most", "some", "such", "than", "then",
];

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
    /// Why it activated: explicit command, admitted capability or task terms.
    pub(crate) reason: String,
    /// Included contributions.
    pub(crate) prompts: Vec<ConditionalPrompt>,
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
pub(crate) fn activate(
    prompts: &[ConditionalPrompt],
    task: &str,
    tools: &[String],
    intent: ActionIntent,
) -> ActivationPlan {
    let task_tokens = tokenize(task);
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
        let reason = activation_reason(first, &task_tokens, task, tools, intent);
        let Some(reason) = reason else {
            for prompt in contributions {
                plan.omissions.push(omission(
                    prompt,
                    if prompt.text.is_empty() {
                        prompt
                            .diagnostic
                            .clone()
                            .unwrap_or_else(|| "empty contribution".to_string())
                    } else {
                        "not relevant to this turn".to_string()
                    },
                ));
            }
            continue;
        };

        let mut extension = ActiveExtension {
            owner: first.owner.clone(),
            name: first.owner_name.clone(),
            version: first.version.clone(),
            trust: first.trust.clone(),
            reason: reason.clone(),
            prompts: Vec::new(),
            bytes: 0,
        };
        for prompt in contributions {
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
            extension.prompts.push(prompt.clone());
        }
        if !extension.prompts.is_empty() {
            plan.active.push(extension);
        }
    }
    plan
}

/// Why an extension activates, if it does.
///
/// Strong signals are explicit and unambiguous: a command the reader invoked,
/// a capability that is already admitted to the turn, and a task that names
/// the extension. Weaker lexical signals are balanced against the turn's
/// intent so a single shared word cannot turn a question into extension work.
fn activation_reason(
    first: &ConditionalPrompt,
    task_tokens: &BTreeSet<String>,
    task: &str,
    tools: &[String],
    intent: ActionIntent,
) -> Option<String> {
    if let Some(command) = first
        .commands
        .iter()
        .find(|command| task.contains(&format!("name=\"{command}\"")))
    {
        return Some(format!("explicit command /{command}"));
    }
    let prefix = format!("{}__", first.tool_namespace);
    if let Some(tool) = tools.iter().find(|tool| tool.starts_with(&prefix)) {
        return Some(format!("admitted capability {tool}"));
    }
    let lower = task.to_lowercase();
    if lower.contains(&first.owner.to_lowercase())
        || lower.contains(&first.owner_name.to_lowercase())
    {
        return Some("task names the extension".to_string());
    }

    let identity = identity_tokens(first);
    let metadata = metadata_tokens(first);
    let identity_hits = identity.intersection(task_tokens).count();
    let metadata_hits = metadata.intersection(task_tokens).count();
    if identity_hits > 0 && intent == ActionIntent::Action {
        return Some(format!(
            "task shares {} with the extension name while asking for work",
            identity
                .intersection(task_tokens)
                .next()
                .cloned()
                .unwrap_or_default()
        ));
    }
    if metadata_hits >= 2 {
        return Some(format!(
            "task shares {metadata_hits} terms with the extension metadata"
        ));
    }
    None
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

/// Identity plus declared contribution and command terms.
fn metadata_tokens(prompt: &ConditionalPrompt) -> BTreeSet<String> {
    let mut terms = identity_tokens(prompt);
    for trigger in &prompt.triggers {
        terms.extend(tokenize(trigger));
    }
    for command in &prompt.commands {
        terms.extend(tokenize(command));
    }
    terms
}

/// Normalize text to lowercase content terms.
///
/// A trailing plural s is folded so "extensions" and "extension" are one term.
/// CJK text has no whitespace and stays one term; that is the same tokenizer
/// contract the memory retrieval uses.
fn tokenize(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter_map(|token| {
            let token = token.to_lowercase();
            let token = token.strip_suffix('s').unwrap_or(&token).to_string();
            (token.chars().count() >= 3 && !STOP_WORDS.contains(&token.as_str())).then_some(token)
        })
        .collect()
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
///
/// The header names the kind, the extension and the active version; the
/// activation reason is model-visible so it can tell a workflow guide from a
/// standing rule. The body is extension text and stays untrusted data.
pub(crate) fn render(active: &ActiveExtension, prompt: &ConditionalPrompt) -> String {
    format!(
        "## Extension {}: {} ({}@{})
Activation: {}
{}",
        prompt.kind, prompt.description, active.owner, active.version, active.reason, prompt.text
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
            triggers: vec![
                "design".to_string(),
                "build".to_string(),
                "validate".to_string(),
                "publish".to_string(),
            ],
            commands: vec![
                "devkit__scaffold".to_string(),
                "devkit__validate".to_string(),
            ],
            tool_namespace: owner.rsplit('.').next().unwrap().to_string(),
            diagnostic: None,
        }
    }

    #[test]
    fn a_greeting_activates_nothing() {
        let prompts = vec![prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            "How to build extensions",
        )];
        let plan = activate(&prompts, "hello 8888", &[], ActionIntent::Unknown);
        assert!(plan.is_empty());
        assert_eq!(plan.omissions.len(), 1);
        assert_eq!(plan.omissions[0].reason, "not relevant to this turn");
    }

    #[test]
    fn extension_work_activates_the_extension_guide() {
        let prompts = vec![prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            "How to build extensions",
        )];
        let plan = activate(
            &prompts,
            "build a new extension package",
            &[],
            ActionIntent::Action,
        );
        assert_eq!(plan.active.len(), 1);
        assert!(plan.active[0].reason.contains("asking for work"));
    }

    #[test]
    fn metadata_overlap_activates_without_an_action_verb() {
        let prompts = vec![prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            "How to build extensions",
        )];
        // "validate" comes from the command id, "extension" from the name.
        let plan = activate(
            &prompts,
            "validate my extension",
            &[],
            ActionIntent::Unknown,
        );
        assert_eq!(plan.active.len(), 1);
        assert!(plan.active[0].reason.contains("shares"));
    }

    #[test]
    fn an_explicit_command_activates_before_the_task_text() {
        let prompts = vec![prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            "How to build extensions",
        )];
        let plan = activate(
            &prompts,
            "<extension_command name=\"devkit__scaffold\">\nScaffold.\n</extension_command>",
            &[],
            ActionIntent::Unknown,
        );
        assert_eq!(plan.active.len(), 1);
        assert_eq!(plan.active[0].reason, "explicit command /devkit__scaffold");
    }

    #[test]
    fn an_admitted_extension_tool_activates_its_guide() {
        let prompts = vec![prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            "How to build extensions",
        )];
        let plan = activate(
            &prompts,
            "do the task",
            &["devkit__scaffold".to_string()],
            ActionIntent::Unknown,
        );
        assert_eq!(plan.active.len(), 1);
        assert_eq!(
            plan.active[0].reason,
            "admitted capability devkit__scaffold"
        );
    }

    #[test]
    fn a_plain_extension_question_is_not_extension_work() {
        let prompts = vec![prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            "How to build extensions",
        )];
        let plan = activate(
            &prompts,
            "what is a browser extension?",
            &[],
            ActionIntent::Explanation,
        );
        assert!(plan.is_empty());
    }

    #[test]
    fn an_unrelated_work_turn_does_not_activate_every_extension() {
        let prompts = vec![prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            "How to build extensions",
        )];
        let plan = activate(&prompts, "fix the failing build", &[], ActionIntent::Action);
        assert!(
            plan.is_empty(),
            "one shared description word is not a topic: {plan:?}"
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
        let plan = activate(
            &[git, devkit],
            "build a new extension package",
            &[],
            ActionIntent::Action,
        );
        assert_eq!(plan.active.len(), 1);
        assert_eq!(plan.active[0].owner, "com.example.devkit");
        assert!(
            plan.omissions
                .iter()
                .any(|omission| omission.owner == "com.example.git"
                    && omission.reason == "not relevant to this turn"),
            "the unrelated extension is reported: {:?}",
            plan.omissions
        );
    }

    #[test]
    fn the_aggregate_budget_omits_the_later_extensions() {
        let boxed = |owner: &str, name: &str| prompt(owner, name, "guide", &"x".repeat(7 * 1024));
        let prompts = vec![
            boxed("com.example.one", "Extension One"),
            boxed("com.example.two", "Extension Two"),
            boxed("com.example.three", "Extension Three"),
            boxed("com.example.four", "Extension Four"),
        ];
        let plan = activate(&prompts, "build an extension", &[], ActionIntent::Action);
        assert!(plan.bytes() <= MAX_ACTIVE_TOTAL);
        assert!(
            plan.omissions
                .iter()
                .any(|omission| omission.reason.contains("aggregate turn budget")),
            "the over-budget contribution is named: {:?}",
            plan.omissions
        );
    }

    #[test]
    fn budgets_omit_with_a_reason() {
        let prompts = vec![prompt(
            "com.example.devkit",
            "Extension DevKit",
            "guide",
            &"x".repeat(MAX_ACTIVE_PER_EXTENSION + 1),
        )];
        let plan = activate(&prompts, "build an extension", &[], ActionIntent::Action);
        assert!(plan.is_empty());
        assert_eq!(plan.omissions.len(), 1);
        assert!(plan.omissions[0]
            .reason
            .contains("per-extension turn budget"));
    }

    #[test]
    fn denied_bodies_are_omitted_with_their_diagnostic() {
        let mut denied = prompt("com.example.devkit", "Extension DevKit", "guide", "");
        denied.diagnostic = Some("capability agent.prompts.provide denied by policy".to_string());
        let plan = activate(&[denied], "build an extension", &[], ActionIntent::Action);
        assert!(plan.is_empty());
        assert!(plan.omissions[0].reason.contains("denied by policy"));
    }
}
