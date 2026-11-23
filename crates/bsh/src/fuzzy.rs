//! The composer's fuzzy matcher: skim's own subsequence scoring, one
//! implementation for every card that filters rows.
//!
//! Both the `/` slash palette and the `@` reference card rank rows with this
//! module, so "partial match" means one thing in the whole composer: a needle
//! `ab` matches a choice `a...b` in order, with skim's bonuses for word
//! starts, gaps and case. The matcher is skim's `SkimMatcherV2` used as a
//! LIBRARY; the picker draws in GPUI, so skim's terminal UI is never started.
//!
//! The module is deliberately GPUI-free: it returns scores and CHARACTER
//! indices, and `crate::ui::highlight` turns those into styled text runs. A
//! character index is not a byte offset (a CJK needle, or a CJK file name,
//! makes the two differ), so the conversion lives here with tests rather than
//! in a view where an off-by-one would only be visible as a mis-coloured glyph.

use std::ops::Range;
use std::sync::OnceLock;

use skim::fuzzy_matcher::FuzzyMatcher;
use skim::fuzzy_matcher::skim::SkimMatcherV2;

/// One candidate that matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hit {
    /// The candidate's index in the caller's own list.
    pub(crate) index: usize,
    /// skim's score; larger is a better match.
    pub(crate) score: i64,
    /// The matched CHARACTER indices in the candidate, in order.
    pub(crate) chars: Vec<usize>,
}

/// The one configured matcher.
///
/// `SkimMatcherV2::default()` is smart case (a lowercase needle ignores case,
/// an uppercase character makes it sensitive) and keeps its pattern/choice
/// caches thread-local, which is exactly the behaviour a reader expects from
/// an fzf prompt. One static is shared by both cards so they cannot drift.
fn matcher() -> &'static SkimMatcherV2 {
    static MATCHER: OnceLock<SkimMatcherV2> = OnceLock::new();
    MATCHER.get_or_init(SkimMatcherV2::default)
}

/// Score one candidate, or none when the needle is not a subsequence of it.
pub(crate) fn score(choice: &str, needle: &str) -> Option<i64> {
    matcher().fuzzy_match(choice, needle)
}

/// Score one candidate and return the matched character indices too.
pub(crate) fn hit(choice: &str, needle: &str) -> Option<(i64, Vec<usize>)> {
    matcher().fuzzy_indices(choice, needle)
}

/// Every candidate that matches, best first.
///
/// Ties keep the caller's order: the input sequence is itself the last
/// tie-break, so a stable list (folders before files, sessions before files)
/// survives a scoring change. An empty needle keeps the caller's order
/// untouched, because a blank query is not a match to rank.
pub(crate) fn rank<'a>(needle: &str, choices: impl IntoIterator<Item = &'a str>) -> Vec<Hit> {
    let mut hits: Vec<Hit> = choices
        .into_iter()
        .enumerate()
        .filter_map(|(index, choice)| {
            let (score, chars) = hit(choice, needle)?;
            Some(Hit {
                index,
                score,
                chars,
            })
        })
        .collect();
    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then(left.index.cmp(&right.index))
    });
    hits
}

/// The byte ranges of the matched characters that fall inside `range`, relative
/// to `range.start`.
///
/// `chars` is what skim returns: indices into `text`'s CHARACTERS. A run is a
/// byte range an editor can style with one `TextRun` (`len` is UTF-8 bytes),
/// and consecutive characters coalesce so a five-letter word is one run rather
/// than five. Characters outside the slice are dropped, which is what lets the
/// caller split one path match across the drawn basename and its parent path.
pub(crate) fn runs_in(text: &str, chars: &[usize], range: Range<usize>) -> Vec<(usize, usize)> {
    // char index -> byte offset, with one extra entry for the end of the text.
    let offsets: Vec<usize> = text
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &char_index in chars {
        let (Some(&start), Some(&end)) = (offsets.get(char_index), offsets.get(char_index + 1))
        else {
            continue;
        };
        if start < range.start || end > range.end {
            continue;
        }
        let start = start - range.start;
        let end = end - range.start;
        match runs.last_mut() {
            Some(last) if last.1 == start => last.1 = end,
            _ => runs.push((start, end)),
        }
    }
    runs
}

/// The byte range of the first case-insensitive occurrence of `needle` in
/// `text`, for a PROSE field.
///
/// Names, paths and titles are matched as subsequences; a one-line description
/// is prose, where a scattered subsequence would match nearly every row (the
/// word "Compact older conversation history" contains m...o...t). A substring
/// is the signal a reader expects from prose, so this helper keeps that half
/// of the filter literal while the name half stays fuzzy.
///
/// An empty result means "no match", not "no highlight": the caller uses it as
/// the match test too. Lowercasing can change a string's byte length (some
/// scripts expand), so a length change is reported as no match rather than as a
/// possibly-wrong range.
pub(crate) fn substring_runs(text: &str, needle: &str) -> Vec<(usize, usize)> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    // skim's smart case: an uppercase character in the needle makes the match
    // case-sensitive. The fuzzy half already behaves that way, and the two
    // halves must agree or one query would find a row by its description while
    // missing it by its name - and an ALL-UPPERCASE needle would still match a
    // mixed-case row through the prose fallback.
    if needle.chars().any(char::is_uppercase) {
        return match text.find(needle) {
            Some(start) => vec![(start, start + needle.len())],
            None => Vec::new(),
        };
    }
    let lowered_text = text.to_lowercase();
    let lowered_needle = needle.to_lowercase();
    if lowered_text.len() != text.len() || lowered_needle.len() != needle.len() {
        return Vec::new();
    }
    match lowered_text.find(&lowered_needle) {
        Some(start) => vec![(start, start + needle.len())],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_needle_matches_across_gaps() {
        // The contract the reader was promised: "ab" matches *a*b*.
        let (_, chars) = hit("a---b", "ab").expect("a...b matches");
        assert_eq!(chars, vec![0, 4]);
        assert!(hit("crates/bsh/src/ui/slash_palette.rs", "supal").is_some());
    }

    #[test]
    fn order_is_respected() {
        assert!(hit("ab", "ba").is_none(), "b must come after a");
        assert!(hit("abc", "ac").is_some());
    }

    #[test]
    fn a_tighter_match_scores_higher() {
        let tight = score("slash_palette.rs", "slash").expect("prefix matches");
        let loose = score("s---l---a---s---h.rs", "slash").expect("scattered matches");
        assert!(tight > loose, "{tight} should beat {loose}");
    }

    #[test]
    fn rank_is_best_first_and_stable() {
        let hits = rank("ab", ["xxab", "ab", "axb"]);
        assert_eq!(hits[0].index, 1, "the exact prefix is first");
        let mut scores: Vec<i64> = hits.iter().map(|hit| hit.score).collect();
        scores.sort_by(|left, right| right.cmp(left));
        assert_eq!(hits.iter().map(|hit| hit.score).collect::<Vec<_>>(), scores);
        // Ties keep the input order: two equal rows keep their indices.
        let tied = rank("x", ["ax", "bx"]);
        assert_eq!(tied.len(), 2);
        assert!(tied[0].index < tied[1].index);
    }

    #[test]
    fn a_blank_needle_ranks_nothing_to_change() {
        let hits = rank("", ["b", "a"]);
        assert_eq!(
            hits.iter().map(|hit| hit.index).collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn runs_are_relative_to_the_slice() {
        let text = "src/ui/slash_palette.rs";
        let (_, chars) = hit(text, "slash").expect("matches");
        let label_start = text.find("slash_palette").expect("label present");
        let runs = runs_in(text, &chars, label_start..text.len());
        assert_eq!(runs, vec![(0, 5)], "the label slice starts at the basename");
    }

    #[test]
    fn cjk_indices_are_characters_not_bytes() {
        // Three CJK characters are nine bytes but three character indices.
        let text = "文件夹/slash.rs";
        let (_, chars) = hit(text, "slash").expect("matches after CJK");
        let runs = runs_in(text, &chars, 0..text.len());
        let byte_start = text.find("slash").expect("present");
        assert_eq!(runs, vec![(byte_start, byte_start + 5)]);
    }

    #[test]
    fn consecutive_characters_coalesce() {
        let text = "abcdef";
        assert_eq!(
            runs_in(text, &[1, 2, 3], 0..text.len()),
            vec![(1, 4)],
            "three adjacent characters are one run"
        );
        assert_eq!(
            runs_in(text, &[1, 2, 4], 0..text.len()),
            vec![(1, 3), (4, 5)]
        );
    }

    #[test]
    fn characters_outside_the_slice_are_dropped() {
        let text = "src/ui/file.rs";
        let runs = runs_in(text, &[0, 7], 7..text.len());
        assert_eq!(runs, vec![(0, 1)], "only the character inside the slice");
    }

    #[test]
    fn an_out_of_range_character_is_ignored() {
        let text = "abc";
        assert!(runs_in(text, &[99], 0..text.len()).is_empty());
    }

    #[test]
    fn a_prose_needle_follows_smart_case() {
        // A lowercase needle ignores case, exactly like the fuzzy half.
        assert_eq!(
            substring_runs("Enter or leave PLAN mode", "plan"),
            vec![(15, 19)]
        );
        assert_eq!(
            substring_runs("Enter or leave plan mode", "plan"),
            vec![(15, 19)]
        );
        // An uppercase needle is case-sensitive: it must not reach a
        // mixed-case row through the prose fallback.
        assert!(substring_runs("Enter or leave plan mode", "PLAN").is_empty());
        assert_eq!(
            substring_runs("Enter or leave PLAN mode", "PLAN"),
            vec![(15, 19)]
        );
        assert!(
            substring_runs("Compact older conversation history", "mot").is_empty(),
            "a scattered subsequence is not a prose match"
        );
    }

    #[test]
    fn a_prose_needle_maps_cjk_bytes() {
        let text = "打开 plan 模式";
        let runs = substring_runs(text, "plan");
        let start = text.find("plan").expect("present");
        assert_eq!(runs, vec![(start, start + 4)]);
    }
}
