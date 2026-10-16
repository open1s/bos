//! The bilingual string table.
//!
//! The strings live in `locales/*.yml` and are compiled in by `rust-i18n`. A
//! key that has no translation returns itself, so a missing string shows up in
//! the window instead of rendering blank.

/// The interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Lang {
    /// English.
    #[default]
    En,
    /// Simplified Chinese.
    Zh,
}

impl Lang {
    /// Switch to the other language.
    /// The language a stored locale code names, defaulting to English.
    pub(crate) fn from_locale(locale: &str) -> Self {
        match locale {
            "zh" => Lang::Zh,
            _ => Lang::En,
        }
    }

    pub(crate) fn toggle(&mut self) {
        *self = match self {
            Lang::En => Lang::Zh,
            Lang::Zh => Lang::En,
        };
    }

    /// The locale tag `rust-i18n` resolves.
    pub(crate) fn locale(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Zh => "zh",
        }
    }

    /// Translate a key into this language.
    pub(crate) fn tr(self, key: &'static str) -> String {
        tr(self, key)
    }
}

/// Translate the key into the language; unknown keys pass through unchanged.
pub(crate) fn tr(lang: Lang, key: &'static str) -> String {
    rust_i18n::t!(key, locale = lang.locale()).to_string()
}

/// Translate the key and substitute a provider name for the provider placeholder.
pub(crate) fn tr_with(lang: Lang, key: &'static str, provider: &str) -> String {
    tr(lang, key).replace("{provider}", provider)
}

/// Translate the key and substitute every `{name}` placeholder.
///
/// The reference's stat and usage strings are templates (`{turns}`,
/// `{count}`, ...), so the measured copy needs its holes filled.
pub(crate) fn tr_args(lang: Lang, key: &'static str, args: &[(&str, String)]) -> String {
    let mut text = tr(lang, key);
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_locales_resolve_the_same_keys() {
        let en = tr(Lang::En, "side.newSession");
        let zh = tr(Lang::Zh, "side.newSession");
        assert_eq!(en, "New Session");
        assert!(!zh.is_empty(), "zh came back empty");
        assert_ne!(zh, en, "zh fell back to english");
    }

    #[test]
    fn navigation_keys_resolve_in_both_locales() {
        for key in [
            "menu.pin",
            "menu.unpin",
            "menu.archive",
            "menu.stop",
            "chat.railTurn",
            "stats.counts",
            "stats.cacheHit",
            "stats.dialog.usageTitle",
            "message.tokensPerSecond",
            "message.turnUsage.count",
            "message.turnUsage.cacheHit",
            "message.turnUsage.input",
            "message.turnUsage.cacheRead",
            "message.turnUsage.output",
        ] {
            assert_ne!(tr(Lang::En, key), key, "{key} missing from en");
            assert_ne!(tr(Lang::Zh, key), key, "{key} missing from zh");
        }
        assert_eq!(
            tr(Lang::En, "chat.railTurn").replace("{turn}", "5"),
            "Turn 5",
            "the rail card fallback is the reference own wording"
        );
        assert_eq!(
            tr(Lang::En, "stats.counts")
                .replace("{turns}", "3")
                .replace("{steps}", "8"),
            "3 turns 8 steps",
            "the pill's counts are the reference own template"
        );
        assert_eq!(
            tr(Lang::Zh, "stats.cacheHit").replace("{percent}", "67"),
            "缓存命中 67%",
            "the usage pill's cache hit is the reference own wording"
        );
    }

    #[test]
    fn unknown_keys_pass_through() {
        assert_eq!(tr(Lang::En, "nope.missing"), "nope.missing");
    }
}
