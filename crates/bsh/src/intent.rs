//! What the reader asked for, as a deterministic local classification.
//!
//! The engine's action contract needs one bit of context the prompt cannot
//! supply reliably: was this turn a request to DO something in the workspace,
//! or a request to EXPLAIN something? The distinction is about the METHOD, not
//! the deliverable: "why does the build fail?" delivers an explanation, but in
//! an active repository it is produced by reading code and running the failing
//! command, so it classifies as `Action`.
//!
//! The classifier is deliberately small, local and reproducible: it never calls
//! a model, never reads the workspace, and only decides whether the engine may
//! spend one corrective retry on a prose-only reply. `Unknown` is the default
//! and the safe answer - a turn classified as unknown gets exactly the prompt's
//! behaviour with no runtime policy.
//!
//! The rules are ordered, and the ordering is the policy:
//!
//! 1. An explicit explanation verb ("explain", "summarize", "解释") is an
//!    explanation even when it mentions an error.
//! 2. A diagnostic question ("why ... fail", "what is wrong", "为什么失败")
//!    or an imperative change verb ("fix", "add", "review", "修复") is action:
//!    the answer has to be built from evidence.
//! 3. A defect report with no clear ask is action.
//! 4. A bare conceptual opener ("what is", "how does", "是什么") is an
//!    explanation.
//!
//! Only the ask sentence is read: a pasted stack trace or a quoted document later
//! in the message cannot flip the classification.

use react::llm::ActionIntent;

/// How much of the message is read. The leading ask is what routes the turn;
/// the rest is usually evidence the reader pasted.
const HEAD_CHARS: usize = 400;

/// Explicit requests for an account. These win over a defect word: "explain
/// this error" asks for a reading, not an investigation.
const EXPLICIT_EXPLAIN: &[&str] = &[
    "explain",
    "describe",
    "summarize",
    "summarise",
    "walk me through",
    "tell me about",
    "teach me",
    "解释",
    "说明",
    "总结",
    "讲讲",
];

/// Openers that ask what something IS or HOW it works. They are an explanation
/// only after the diagnostic and action rules declined: "what is wrong with the
/// build" is a diagnostic, while "what is a monad" is a concept.
const CONCEPT_OPENERS: &[&str] = &[
    "what is",
    "what are",
    "how does",
    "how did",
    "是什么",
    "怎么理解",
    "是什么原理",
];

/// Words that name a defect or a diagnostic task. With a question word they
/// make a repository question actionable; alone they are a defect report.
const DEFECT: &[&str] = &[
    "fail",
    "fails",
    "failed",
    "failing",
    "failure",
    "error",
    "errors",
    "crash",
    "crashes",
    "broken",
    "doesn't work",
    "does not work",
    "not working",
    "wrong",
    "slow",
    "stuck",
    "hang",
    "hangs",
    "bug",
    "报错",
    "失败",
    "崩溃",
    "不工作",
    "很慢",
    "卡住",
    "问题",
];

/// Question forms that ask for a cause. A cause is established by evidence.
const WHY: &[&str] = &[
    "why",
    "how come",
    "what's wrong",
    "what is wrong",
    "what happened",
    "为什么",
    "为啥",
    "怎么回事",
    "什么原因",
];

/// Verbs that ask for work in the workspace, including review and delivery.
const ACTION: &[&str] = &[
    "fix",
    "implement",
    "add",
    "change",
    "update",
    "refactor",
    "rename",
    "remove",
    "delete",
    "migrate",
    "make",
    "write",
    "create",
    "build",
    "run",
    "test",
    "debug",
    "diagnose",
    "investigate",
    "troubleshoot",
    "reproduce",
    "optimize",
    "optimise",
    "optimizing",
    "speed up",
    "deploy",
    "review",
    "audit",
    "修复",
    "实现",
    "添加",
    "增加",
    "加一个",
    "加个",
    "写一个",
    "写个",
    "修改",
    "改成",
    "重构",
    "重命名",
    "删除",
    "去掉",
    "迁移",
    "优化",
    "调试",
    "排查",
    "检查",
    "审查",
    "部署",
];

/// Classify one reader message.
pub(crate) fn classify(text: &str) -> ActionIntent {
    let head: String = text.trim().chars().take(HEAD_CHARS).collect();
    if head.is_empty() {
        return ActionIntent::Unknown;
    }
    let head = head.to_lowercase();

    let head = padded(&head);
    if matches_any(&head, EXPLICIT_EXPLAIN) {
        return ActionIntent::Explanation;
    }
    if matches_any(&head, WHY) && matches_any(&head, DEFECT) {
        return ActionIntent::Action;
    }
    if matches_any(&head, ACTION) {
        return ActionIntent::Action;
    }
    if matches_any(&head, DEFECT) {
        return ActionIntent::Action;
    }
    if matches_any(&head, CONCEPT_OPENERS) {
        return ActionIntent::Explanation;
    }
    ActionIntent::Unknown
}

/// Pad a lowercase message with spaces and turn punctuation into spaces, so an
/// ASCII keyword matches a whole word: "add" must not match "address".
///
/// Unicode letters (Chinese among them) pass through unchanged, which keeps a
/// CJK substring working without a tokenizer.
fn padded(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '\'' {
            out.push(ch);
        } else {
            out.push(' ');
        }
    }
    out.push(' ');
    out
}

fn matches_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        if needle.is_ascii() {
            haystack.contains(&format!(" {needle} "))
        } else {
            haystack.contains(needle)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagnostic_question_is_action_not_explanation() {
        for prompt in [
            "why does login fail?",
            "why is the build slow?",
            "why does the test crash",
            "what's wrong with the sync loop?",
            "what is wrong with the build?",
            "how do I fix the build?",
            "为什么构建失败",
            "为什么登录报错",
        ] {
            assert_eq!(classify(prompt), ActionIntent::Action, "{prompt}");
        }
    }

    #[test]
    fn an_explicit_explanation_ask_stays_an_explanation() {
        for prompt in [
            "explain how the retry works",
            "explain this error message",
            "what is a monad?",
            "what are the tradeoffs of a cache?",
            "how does caching work?",
            "summarize the architecture",
            "summarize this error",
            "解释一下这个错误",
            "说明一下授权流程",
        ] {
            assert_eq!(classify(prompt), ActionIntent::Explanation, "{prompt}");
        }
    }

    #[test]
    fn an_imperative_is_action() {
        for prompt in [
            "fix the retry logic",
            "add a test for the parser",
            "review this change",
            "implement the cache eviction",
            "修复登录问题",
            "给解析器加一个测试",
        ] {
            assert_eq!(classify(prompt), ActionIntent::Action, "{prompt}");
        }
    }

    #[test]
    fn small_talk_and_bare_questions_have_no_policy() {
        for prompt in ["hello", "thanks!", "what time is it?", ""] {
            assert_eq!(classify(prompt), ActionIntent::Unknown, "{prompt}");
        }
    }

    /// The ask sentence is what routes the turn: pasted evidence later in the
    /// message cannot reclassify it.
    #[test]
    fn pasted_evidence_after_the_ask_does_not_flip_the_intent() {
        let prompt = format!(
            "explain how the cache works\n\n{}",
            "error: cache miss\n".repeat(80)
        );
        assert_eq!(classify(&prompt), ActionIntent::Explanation);
    }

    /// A long pasted document is bounded: only the leading ask is read, so a
    /// classification cost cannot grow with a pasted log.
    #[test]
    fn only_the_leading_ask_is_read() {
        let filler = "x".repeat(HEAD_CHARS);
        assert_eq!(
            classify(&format!("hello {filler} fix it")),
            ActionIntent::Unknown
        );
    }
}
