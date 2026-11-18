//! Deterministic extraction of decisions, questions and next actions from a
//! reasoning stream.
//!
//! The extractor is intentionally cheap and explainable: it reads the tail of
//! one reasoning step, splits it into sentences, and matches the linguistic
//! markers a model uses when it commits to a direction ("the fix is", "I will",
//! "because", "next step"). It never calls a second model, and it only ever
//! produces [`super::ThinkingConfidence::Inferred`] items: corroboration is the
//! tool-result path's job.

use super::summary::{ThinkingConfidence, ThinkingDecision, ThinkingOpenQuestion, ThinkingOrigin};
use super::{clip_chars, one_line, QUESTION_CHARS, REASONING_TAIL_CHARS};

/// Markers that introduce a judgment or a chosen direction.
const JUDGMENT_CUES: &[&str] = &[
    "the fix is",
    "the approach is",
    "the plan is",
    "the cause is",
    "is the cause",
    "the root cause",
    "the bug is",
    "is the bug",
    "the issue is",
    "is the issue",
    "the problem is",
    "i will use",
    "we will use",
    "i'll use",
    "we'll use",
    "let's use",
    "we should use",
    "i should use",
    "决定",
    "方案是",
    "采用",
    "根因",
    "原因是",
    "问题在于",
    "应该使用",
];

/// Markers that introduce an intended action.
const INTENT_CUES: &[&str] = &[
    "i will ",
    "i'll ",
    "i am going to ",
    "i'm going to ",
    "we will ",
    "we'll ",
    "let's ",
    "let me ",
    "i should ",
    "we should ",
    "i need to ",
    "we need to ",
    "我将",
    "我会",
    "我们需要",
    "我需要",
    "接下来",
    "下一步",
    "然后我",
];

/// Markers that introduce the next concrete action.
const NEXT_CUES: &[&str] = &[
    "next, i will ",
    "next i will ",
    "next i'll ",
    "next, i'll ",
    "then i will ",
    "then i'll ",
    "now i will ",
    "i will now ",
    "next step:",
    "next step ",
    "接下来",
    "下一步",
    "然后我",
    "现在需要",
    "i will ",
    "i'll ",
    "we will ",
    "we'll ",
];

/// Sentence openings that are already an action.
const IMPERATIVES: &[&str] = &[
    "run ",
    "test ",
    "check ",
    "read ",
    "open ",
    "fix ",
    "add ",
    "update ",
    "implement ",
    "verify ",
    "try ",
    "search ",
    "inspect ",
    "look ",
    "write ",
    "remove ",
    "move ",
    "build ",
    "install ",
    "rerun ",
    "re-run ",
    "debug ",
    "compare ",
    "trace ",
    "运行",
    "检查",
    "阅读",
    "打开",
    "修复",
    "添加",
    "更新",
    "实现",
    "验证",
    "尝试",
    "搜索",
    "查看",
    "编写",
    "删除",
    "构建",
    "安装",
];

/// Markers of an unresolved question, even without a question mark.
const QUESTION_CUES: &[&str] = &[
    "need to check",
    "need to verify",
    "not sure",
    "unclear",
    "open question",
    "to confirm",
    "待确认",
    "需要确认",
    "尚未",
    "不确定",
    "是否",
    "还需",
];

/// Split `text` into sentence-like units, keeping terminal punctuation.
pub(crate) fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    for (index, ch) in chars.iter().enumerate() {
        current.push(*ch);
        let boundary = match ch {
            '\n' | '。' | '！' | '？' | '!' | '?' | ';' | '；' => true,
            '.' => chars.get(index + 1).is_none_or(|next| next.is_whitespace()),
            _ => false,
        };
        if boundary {
            push_sentence(&mut out, &mut current);
        }
    }
    push_sentence(&mut out, &mut current);
    out
}

fn push_sentence(out: &mut Vec<String>, current: &mut String) {
    let trimmed = strip_bullet(current.trim());
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    current.clear();
}

fn strip_bullet(text: &str) -> &str {
    let text = text.trim_start_matches(['-', '*', '•', ' ']).trim_start();
    // "1. " / "1) " list markers.
    let digits: usize = text.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &text[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return rest.trim();
        }
    }
    text
}

fn is_question(sentence: &str) -> bool {
    let lower = sentence.to_lowercase();
    sentence.trim_end().ends_with('?')
        || sentence.trim_end().ends_with('？')
        || QUESTION_CUES.iter().any(|cue| lower.contains(cue))
}

/// Split a statement from the reason attached to it.
fn split_rationale(sentence: &str) -> (String, Option<String>) {
    let lower = sentence.to_lowercase();
    for marker in [" because ", " since "] {
        if let Some(index) = lower.find(marker) {
            if index > 0 {
                let statement = sentence[..index].trim().to_string();
                let rationale = sentence[index + marker.len()..].trim().to_string();
                if !statement.is_empty() && !rationale.is_empty() {
                    return (statement, Some(rationale));
                }
            }
        }
    }
    for marker in ["因为", "由于"] {
        if let Some(index) = sentence.find(marker) {
            if index > 0 {
                let statement = sentence[..index].trim().to_string();
                let rationale = sentence[index + marker.len()..]
                    .trim_start_matches(['，', ',', ' '])
                    .trim()
                    .to_string();
                if !statement.is_empty() && !rationale.is_empty() {
                    return (statement, Some(rationale));
                }
            }
        }
    }
    (sentence.trim().to_string(), None)
}

fn has_cue(lower: &str, cues: &[&str]) -> bool {
    cues.iter().any(|cue| lower.contains(cue))
}

/// Extract the decisions one reasoning step stated.
///
/// Every result is `Inferred`: reasoning alone cannot verify its own claims.
pub(crate) fn extract_decisions(text: &str, step: usize) -> Vec<ThinkingDecision> {
    let mut out = Vec::new();
    for sentence in split_sentences(text) {
        if sentence.chars().count() < 6 || is_question(&sentence) {
            continue;
        }
        let lower = sentence.to_lowercase();
        if !has_cue(&lower, JUDGMENT_CUES) && !has_cue(&lower, INTENT_CUES) {
            continue;
        }
        let (statement, rationale) = split_rationale(&sentence);
        let statement = clip_chars(&one_line(&statement), super::ITEM_CHARS);
        if statement.chars().count() < 6 {
            continue;
        }
        out.push(ThinkingDecision {
            statement,
            rationale: rationale.map(|text| clip_chars(&one_line(&text), super::ITEM_CHARS)),
            confidence: ThinkingConfidence::Inferred,
            origin: ThinkingOrigin::Reasoning,
            evidence: Vec::new(),
            step,
            active: true,
            superseded_by: None,
        });
        if out.len() >= 4 {
            break;
        }
    }
    out
}

/// Extract the questions one reasoning step left open.
pub(crate) fn extract_questions(text: &str, step: usize) -> Vec<ThinkingOpenQuestion> {
    let mut out = Vec::new();
    for sentence in split_sentences(text) {
        if !is_question(&sentence) {
            continue;
        }
        let question = clip_chars(&one_line(strip_bullet(&sentence)), QUESTION_CHARS);
        if question.chars().count() < 8 {
            continue;
        }
        out.push(ThinkingOpenQuestion {
            question,
            step,
            resolved: false,
        });
        if out.len() >= 4 {
            break;
        }
    }
    out
}

/// Extract the one next action a reasoning step named, if it named one.
pub(crate) fn extract_next_action(text: &str) -> Option<String> {
    let sentences = split_sentences(text);
    for sentence in sentences.iter().rev() {
        if is_question(sentence) {
            continue;
        }
        let lower = sentence.to_lowercase();
        for cue in NEXT_CUES {
            if let Some(index) = lower.find(cue) {
                let tail = sentence[index + cue.len()..]
                    .trim_start_matches([':', '：', ',', '，', ' ', '-'])
                    .trim();
                if tail.chars().count() >= 4 {
                    // The action is the sentence's statement; its reason is not
                    // part of what the next step does.
                    let (action, _) = split_rationale(tail);
                    return Some(clip_chars(&one_line(&action), super::ITEM_CHARS));
                }
            }
        }
    }
    for sentence in sentences.iter().rev() {
        if is_question(sentence) {
            continue;
        }
        let stripped = strip_bullet(sentence).to_lowercase();
        if IMPERATIVES.iter().any(|verb| stripped.starts_with(verb)) {
            return Some(clip_chars(&one_line(sentence), super::ITEM_CHARS));
        }
    }
    None
}

/// Whether `text` reads as a failed tool result.
pub(crate) fn looks_failed(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains("0 failed")
        || lower.contains("test result: ok")
        || lower.contains("all tests passed")
        || lower.contains("build succeeded")
        || lower.contains("\"success\": true")
    {
        return false;
    }
    text.starts_with("Error:")
        || text.contains("\"success\": false")
        || text.contains("\"success\":false")
        || lower.contains("test result: failed")
        || lower.contains("error[")
        || lower.contains("panicked at")
        || lower.contains("build failed")
        || lower.contains("command failed")
        || lower.contains("fatal:")
        || lower.contains("traceback (most recent call last)")
}

/// Whether `text` is a test or build result that verifies the work.
pub(crate) fn looks_verifying(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("test result:")
        || lower.contains("tests passed")
        || lower.contains("build success")
        || lower.contains("cargo test")
        || lower.contains("pytest")
        || lower.contains("all checks passed")
}

/// Whether a statement is negated.
pub(crate) fn is_negated(text: &str) -> bool {
    let lower = format!(" {} ", text.to_lowercase());
    lower.contains(" not ")
        || lower.contains(" no ")
        || lower.contains(" never ")
        || lower.contains(" without ")
        || lower.contains(" isn't ")
        || lower.contains(" doesn't ")
        || lower.contains(" cannot ")
        || text.contains('不')
        || text.contains("没有")
        || text.contains("并非")
        || text.contains("不是")
}

/// The leading line of a result, for evidence and questions.
pub(crate) fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| clip_chars(line, 160))
        .unwrap_or_default()
}

/// Parse only the bounded tail of a reasoning stream.
pub(crate) fn bounded_tail(text: &str) -> &str {
    let count = text.chars().count();
    if count <= REASONING_TAIL_CHARS {
        return text;
    }
    let skip = count - REASONING_TAIL_CHARS;
    let byte = text
        .char_indices()
        .nth(skip)
        .map(|(index, _)| index)
        .unwrap_or(0);
    &text[byte..]
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_tail, extract_decisions, extract_next_action, extract_questions, is_negated,
        looks_failed, looks_verifying, split_rationale, split_sentences,
    };
    use crate::thinking::ThinkingConfidence;

    #[test]
    fn sentences_split_on_english_and_chinese_punctuation() {
        let parts = split_sentences("First. Second?\n第三。第四！");
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[2], "第三。");
    }

    #[test]
    fn a_reason_is_split_from_its_statement() {
        let (statement, rationale) =
            split_rationale("the parser is the cause because the test starts there");
        assert_eq!(statement, "the parser is the cause");
        assert_eq!(rationale.as_deref(), Some("the test starts there"));
    }

    #[test]
    fn decisions_are_inferred_with_their_rationale() {
        let decisions = extract_decisions(
            "The fix is to reuse the existing lock because it already coordinates writers.",
            2,
        );
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].confidence, ThinkingConfidence::Inferred);
        assert_eq!(decisions[0].step, 2);
        assert!(decisions[0]
            .rationale
            .as_deref()
            .is_some_and(|text| text.contains("coordinates")));
    }

    #[test]
    fn questions_and_next_actions_are_extracted() {
        let text = "I need to confirm whether the cache is stale. Next step: run the memory tests.";
        let questions = extract_questions(text, 1);
        assert!(questions
            .iter()
            .any(|question| question.question.contains("cache")));
        assert_eq!(
            extract_next_action(text).as_deref(),
            Some("run the memory tests.")
        );
    }

    #[test]
    fn chinese_reasoning_yields_decisions_questions_and_actions() {
        let decisions = extract_decisions("我决定采用现有锁，因为已经有协调机制。", 1);
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].statement.contains("采用现有锁"));
        assert!(decisions[0]
            .rationale
            .as_deref()
            .is_some_and(|text| text.contains("协调机制")));

        let questions = extract_questions("是否需要确认缓存？", 1);
        assert_eq!(questions.len(), 1);

        assert_eq!(
            extract_next_action("接下来，运行解析器测试。").as_deref(),
            Some("运行解析器测试。")
        );
    }

    #[test]
    fn negative_and_failed_results_are_recognized() {
        assert!(is_negated("the cache is not the cause"));
        assert!(looks_failed("Error: permission denied"));
        assert!(looks_failed("test result: FAILED. 2 passed; 1 failed"));
        assert!(!looks_failed("test result: ok. 12 passed; 0 failed"));
        assert!(looks_verifying("test result: ok. 12 passed"));
    }

    #[test]
    fn the_parser_keeps_only_the_tail_of_a_long_stream() {
        let long = format!("{}{}", "a".repeat(20_000), "tail");
        assert_eq!(
            bounded_tail(&long).chars().count(),
            super::REASONING_TAIL_CHARS
        );
        assert!(bounded_tail(&long).ends_with("tail"));
    }
}
