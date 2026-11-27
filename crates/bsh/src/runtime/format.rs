//! Display formatting shared by every interface.
//!
//! These are the reference's own number spellings, moved out of the desktop's
//! state so the terminal renders the same figure: a count grouped in threes, a
//! scaled token count, a cache-hit percentage and a context occupancy share.
//! They are pure functions of numbers - no session, no settings, no view - so a
//! terminal and a window cannot drift apart on the arithmetic.

/// The reference's display-ready cache-hit share, as text.
///
/// Its `formatCacheHitPercent` rounds to whole percent with ties up, but never
/// lets a partial hit read as a full one: when rounding would reach 100 it
/// widens the precision until the miss shows, so 199 of 200 reads 99.5. No
/// prompt input at all reads as unknown rather than zero.
pub(crate) fn cache_hit_percent(cache_read: u64, prompt: u64) -> Option<String> {
    if prompt == 0 {
        return None;
    }
    let missed = prompt.saturating_sub(cache_read);
    if missed == 0 {
        return Some("100".to_string());
    }
    let units = (cache_read as u128 * 200 + prompt as u128) / (prompt as u128 * 2);
    if units < 100 {
        return Some(units.to_string());
    }
    // A partial hit whose whole percent rounds to 100: walk the precision out
    // until the remaining miss shows, the way the reference does.
    let mut places = 1usize;
    let mut gap = missed as u128 * 200;
    let tens = (prompt / 10) as u128;
    while gap <= tens {
        gap *= 10;
        places += 1;
    }
    let ones = (prompt % 10) as u128;
    let mut loss = 5u64;
    for candidate in 1..5u64 {
        let factor = candidate as u128 * 2 + 1;
        if gap <= factor * tens + factor * ones / 10 {
            loss = candidate;
            break;
        }
    }
    Some(format!("99.{}{}", "9".repeat(places - 1), 10 - loss))
}

/// A token count with the reference's own digit grouping: 17,734.
pub(crate) fn exact_tokens(tokens: u64) -> String {
    let digits = tokens.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Compact token counts the way the reference's own `formatTokens` reads them:
/// 947, 12.3K, 518K, 1.2M.
///
/// Under a thousand it prints the count; above that it scales to K or M, where
/// a scaled value of 100 or more rounds to a whole number and a smaller one
/// keeps a single decimal. The reference localizes the two suffix keys, but its
/// own doc comment pins the shipped values to K and M.
pub(crate) fn human_tokens(tokens: u64) -> String {
    fn scaled(value: u64, scale: u64) -> String {
        let whole = value / scale;
        let remainder = value % scale;
        if whole >= 100 {
            return (whole + u64::from(remainder * 2 >= scale)).to_string();
        }
        let tenths = whole * 10 + (remainder * 10 + scale / 2) / scale;
        if tenths.is_multiple_of(10) {
            (tenths / 10).to_string()
        } else {
            format!("{}.{}", tenths / 10, tenths % 10)
        }
    }
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{}K", scaled(tokens, 1_000))
    } else {
        format!("{}M", scaled(tokens, 1_000_000))
    }
}

/// The reference's own rounding of an occupancy share, capped at 100.
///
/// Its `contextPressure` computes `Math.min(100, Math.round(used / window *
/// 100))`, so a 45.4% occupancy reads 45% in the ring and in the panel alike. A
/// missing budget or an unmeasured prompt reads as unknown rather than zero.
pub(crate) fn context_share(prompt: u64, budget: u64) -> Option<u64> {
    (budget > 0 && prompt > 0)
        .then(|| ((prompt as f64 / budget as f64) * 100.0).round().min(100.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::{cache_hit_percent, context_share, exact_tokens, human_tokens};

    #[test]
    fn token_counts_switch_units_at_a_thousand() {
        assert_eq!(human_tokens(0), "0");
        assert_eq!(human_tokens(947), "947");
        assert_eq!(human_tokens(12_345), "12.3K");
        assert_eq!(human_tokens(1_000), "1K");
        assert_eq!(human_tokens(517_500), "518K", "a scaled 100+ rounds");
        assert_eq!(human_tokens(69_200_000), "69.2M");
    }

    #[test]
    fn exact_counts_group_in_threes() {
        assert_eq!(exact_tokens(0), "0");
        assert_eq!(exact_tokens(947), "947");
        assert_eq!(exact_tokens(17_734), "17,734");
        assert_eq!(exact_tokens(1_234_567), "1,234,567");
    }

    #[test]
    fn the_cache_hit_never_rounds_a_partial_hit_to_full() {
        assert_eq!(cache_hit_percent(67, 100).as_deref(), Some("67"));
        assert_eq!(
            cache_hit_percent(2, 3).as_deref(),
            Some("67"),
            "ties round up"
        );
        assert_eq!(cache_hit_percent(199, 200).as_deref(), Some("99.5"));
        assert_eq!(cache_hit_percent(999, 1000).as_deref(), Some("99.9"));
        assert_eq!(cache_hit_percent(200, 200).as_deref(), Some("100"));
        assert_eq!(cache_hit_percent(0, 0), None);
        assert_eq!(cache_hit_percent(0, 100).as_deref(), Some("0"));
    }

    /// The occupancy share rounds like the reference's own Math.round and caps
    /// at 100.
    #[test]
    fn the_context_share_rounds_like_the_reference() {
        assert_eq!(context_share(454, 1_000), Some(45));
        assert_eq!(context_share(455, 1_000), Some(46));
        assert_eq!(context_share(999, 1_000), Some(100));
        // A prompt past its budget still reads full, never 101%.
        assert_eq!(context_share(50_000, 1_000), Some(100));
        assert_eq!(context_share(0, 1_000), None);
        assert_eq!(context_share(10, 0), None);
    }
}
