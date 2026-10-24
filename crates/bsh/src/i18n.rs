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

    /// The hero's line is the brand's own, not the reference's placeholder:
    /// "See Beyond. Stay True." and its Chinese counterpart.
    #[test]
    fn the_hero_headline_is_the_brand_line() {
        assert_eq!(tr(Lang::En, "hero.headline"), "See Beyond. Stay True.");
        assert_eq!(tr(Lang::Zh, "hero.headline"), "洞察未见，忠于所托");
    }

    #[test]
    fn both_locales_resolve_the_same_keys() {
        let en = tr(Lang::En, "side.newSession");
        let zh = tr(Lang::Zh, "side.newSession");
        assert_eq!(en, "New Session");
        assert!(!zh.is_empty(), "zh came back empty");
        assert_ne!(zh, en, "zh fell back to english");
    }

    /// The settings footer's save confirmation is the exact copy both
    /// languages show.
    ///
    /// These two keys already existed - Save wrote the success one into the
    /// statusbar behind the dialog - so the toast reuses them rather than
    /// adding near-duplicates. The failure string ends in a colon because the
    /// write error is appended to it.
    #[test]
    fn the_settings_save_confirmation_carries_both_languages() {
        assert_eq!(tr(Lang::En, "settings.saved"), "Settings saved");
        assert_eq!(tr(Lang::Zh, "settings.saved"), "设置已保存");
        assert_eq!(
            tr(Lang::En, "settings.saveFailed"),
            "Could not save settings:"
        );
        assert_eq!(tr(Lang::Zh, "settings.saveFailed"), "无法保存设置：");
    }

    /// The two locale files carry exactly the same keys.
    ///
    /// `tr` renders a missing key as itself, so a key added to one file and
    /// forgotten in the other is invisible to the compiler and visible only to
    /// the reader in whichever language did not get the line. This reads the
    /// source files, not the compiled tables, because rust-i18n offers no key
    /// enumeration.
    #[test]
    fn the_two_locale_files_carry_the_same_keys() {
        fn keys(text: &str) -> std::collections::BTreeSet<String> {
            text.lines()
                .filter_map(|line| {
                    let line = line.trim();
                    let rest = line.strip_prefix('"')?;
                    let (key, tail) = rest.split_once('"')?;
                    tail.trim_start().starts_with(':').then(|| key.to_string())
                })
                .collect()
        }
        let en = keys(include_str!("../locales/en.yml"));
        let zh = keys(include_str!("../locales/zh.yml"));
        assert_eq!(
            en.difference(&zh).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "keys missing from zh"
        );
        assert_eq!(
            zh.difference(&en).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "keys missing from en"
        );
    }

    /// Every compact outcome is the shipped command's own wording, in both
    /// locales, with the UI rows the reference localizes taken from its zh
    /// dictionary rather than translated by hand.
    #[test]
    fn compact_outcomes_use_the_reference_wording() {
        assert_eq!(
            tr(Lang::En, "slash.compactBusy"),
            "Compaction is unavailable because this conversation has an active compaction, or its agent is not idle."
        );
        assert_eq!(
            tr(Lang::En, "slash.nothingToCompact"),
            "No compactable history yet."
        );
        assert_eq!(
            tr(Lang::En, "slash.compacted"),
            "Compacted {count} history items (~{tokens} tokens)."
        );
        assert_eq!(
            tr(Lang::En, "slash.compactUsage"),
            "Usage: /compact (no arguments)"
        );
        assert_eq!(
            tr(Lang::En, "slash.compactSummary"),
            "Compaction could not produce a useful summary. The attempt is recorded in the session log."
        );
        assert_eq!(
            tr(Lang::En, "slash.compactCancelled"),
            "Compaction cancelled."
        );
        // Each reachable failure class has its own line: one generic sentence
        // was hiding provider errors, empty answers and grow-gate refusals.
        assert_eq!(
            tr(Lang::En, "slash.compactEmpty"),
            "The summarizer returned an empty summary; the history is unchanged."
        );
        assert_eq!(
            tr(Lang::Zh, "slash.compactEmpty"),
            "摘要模型返回了空内容；历史保持不变。"
        );
        assert!(tr(Lang::En, "slash.compactProvider").contains("{error}"));
        assert!(tr(Lang::Zh, "slash.compactProvider").contains("{error}"));
        assert!(tr(Lang::En, "slash.compactNotSmaller").contains("{framed}"));
        assert!(tr(Lang::Zh, "slash.compactNotSmaller").contains("{span}"));
        assert_ne!(
            tr(Lang::En, "slash.compactNoModel"),
            tr(Lang::En, "slash.compactSummary"),
            "no-model is not a summary failure"
        );
        assert_eq!(
            tr(Lang::En, "slash.compactChanged"),
            "The history selected for compaction changed before it could be replaced. The attempt is recorded in the session log."
        );
        assert_eq!(
            tr(Lang::En, "slash.compactCommit"),
            "Compaction did not finish cleanly; some session history may have changed. Inspect the current session state before retrying."
        );
        assert_eq!(
            tr(Lang::En, "slash.compactPersistence"),
            "Compaction finished, but the session could not be saved."
        );
        // The two rows the reference ships a Chinese dictionary for.
        assert_eq!(tr(Lang::Zh, "slash.compacting"), "正在压缩…");
        assert_eq!(
            tr(Lang::Zh, "slash.compacted"),
            "已压缩 {count} 条历史记录（约 {tokens} tokens）"
        );
        assert_eq!(
            tr(Lang::En, "msg.compaction.running"),
            "Compacting context…"
        );
        assert_eq!(tr(Lang::Zh, "msg.compaction.running"), "正在压缩…");
        assert_eq!(tr(Lang::Zh, "msg.compaction.expand"), "点击查看压缩摘要");
        assert_eq!(tr(Lang::Zh, "msg.compaction.unavailable"), "压缩摘要不可用");
        assert_eq!(tr(Lang::En, "msg.compaction.commandTitle"), "compact");
        assert_eq!(tr(Lang::Zh, "msg.compaction.commandTitle"), "compact");
        // The finished line carries the two counts the shipped command
        // reports: replaced history items and the shadowed span's tokens.
        assert_eq!(
            tr_args(
                Lang::En,
                "slash.compacted",
                &[("count", "4".to_string()), ("tokens", "500".to_string())],
            ),
            "Compacted 4 history items (~500 tokens)."
        );
        assert_eq!(
            tr_args(
                Lang::Zh,
                "slash.compacted",
                &[("count", "4".to_string()), ("tokens", "500".to_string())],
            ),
            "已压缩 4 条历史记录（约 500 tokens）"
        );
        assert_eq!(
            tr_args(
                Lang::En,
                "msg.compaction.completed",
                &[("items", "4".to_string()), ("tokens", "500".to_string())],
            ),
            "Compacted 4 history items (~500 tokens)"
        );
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
            // The terminal's policy surfaces: the refused input, the output
            // marker, the compact blocked count and the policy parse error.
            "right.terminal.inputRefused",
            "right.terminal.outputBlocked",
            "right.terminal.policyBlocked",
            "right.terminal.policyError",
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

    /// A key that is missing is INVISIBLE to the compiler but visible to the
    /// user: `tr` returns the key itself, so a namespace slip ships the raw text
    /// "settings.presetReadOnly" as a button label. That is not a hypothesis -
    /// the composer chip did exactly that for a round, and the shortcuts row
    /// asked for `settings.shortcutsDesc` where the key is
    /// `settings.shortcutsDescription`. Both were caught only by resolving the
    /// key at runtime, which is what this does for the whole General pane.
    ///
    /// The list is the pane's own vocabulary, including the keys built
    /// dynamically (`appearance_row` picks a cube label by theme mode and the
    /// permission rows go through `permission_label_key`), which is exactly the
    /// class the compiler cannot gather. Keep it in step when a row is added.
    #[test]
    fn general_pane_keys_resolve_in_both_locales() {
        for key in [
            // The pane frame and its own rows.
            "settings.general",
            "settings.generalHint",
            "settings.general.currentVersion",
            "settings.openConfig",
            "settings.revealFailed",
            // Permission, including the three labels the chip builds by mode.
            "settings.permission",
            "permission.description",
            "permission.presetReadOnly",
            "permission.presetWorkspaceWrite",
            "permission.presetFullAccess",
            "permission.consequenceReadOnly",
            "permission.consequenceWorkspace",
            "permission.consequenceFull",
            "permission.consequenceFullShell",
            "permission.cardReadOnly",
            "permission.cardWorkspace",
            "permission.cardFull",
            "permission.cardFullShell",
            "permission.cardCapped",
            "permission.keptShort",
            // The network clause: Read Only's fixed refusal sentence, then the
            // actors, the two sentence shapes and the five derived reach states.
            "permission.netReadOnly",
            "permission.netActorsCommands",
            "permission.netActorsTools",
            "permission.netActors",
            "permission.netGeneric",
            "permission.netReachFree",
            "permission.netReachWorkspace",
            "permission.netReachPreset",
            "permission.netDenyWorkspace",
            "permission.netDenyPreset",
            "permission.confirmTitle",
            "permission.confirmDescription",
            "permission.confirmCancel",
            "permission.confirmEnable",
            // Language and appearance, whose cube labels are chosen by mode.
            "settings.language",
            "settings.appearance",
            "settings.modeLight",
            "settings.modeDark",
            "settings.modeSystem",
            "settings.fontSize",
            "settings.fontSizeHint",
            "settings.fontSizeUnit",
            // Work details.
            "settings.transcriptTitle",
            "settings.transcriptDescription",
            "settings.transcriptCompact",
            "settings.transcriptStandard",
            "settings.transcriptDetailed",
            "settings.transcriptVerbose",
            // The coding-view switch.
            "settings.developerToolsTitle",
            "settings.developerToolsDescription",
            "settings.developerToolsError",
            // Shortcuts - the row whose hint was mis-namespaced.
            "settings.shortcutsTitle",
            "settings.shortcutsDescription",
            "settings.shortcutsView",
            "settings.shortcutsUnbound",
            // Link opening and the busy-state Enter behavior.
            "settings.linksTitle",
            "settings.linksDescription",
            "settings.linksSidebar",
            "settings.linksNewTab",
            "settings.enterTitle",
            "settings.enterDescription",
            "settings.enterQueue",
            "settings.enterSteer",
            // Performance.
            "settings.performanceTitle",
            "settings.performanceDescription",
            "settings.performanceCompact",
            "settings.performanceDetailed",
        ] {
            assert_ne!(tr(Lang::En, key), key, "{key} missing from en");
            assert_ne!(tr(Lang::Zh, key), key, "{key} missing from zh");
        }
    }

    /// The Vault pane's whole vocabulary resolves in both locales, including the
    /// dynamic refusal keys the backend hands it.
    #[test]
    fn vault_pane_keys_resolve_in_both_locales() {
        for key in [
            "settings.vault",
            "settings.vaultHint",
            "settings.vaultExplain",
            "settings.vaultBusy",
            "settings.vaultPassphrase",
            "settings.vaultPassphrasePlaceholder",
            "settings.vaultConfirm",
            "settings.vaultConfirmPlaceholder",
            "settings.vaultUnlock",
            "settings.vaultCreate",
            "settings.vaultCreateHint",
            "settings.vaultLockedHint",
            "settings.vaultAdminLockedHint",
            "settings.vaultStrengthShort",
            "settings.vaultStrengthFair",
            "settings.vaultStrengthStrong",
            "settings.vaultUnlocked",
            "settings.vaultEntryCount",
            "settings.vaultEntries",
            "settings.vaultEmpty",
            "settings.vaultCreated",
            "settings.vaultReveal",
            "settings.vaultHide",
            "settings.vaultCopy",
            "settings.vaultCopied",
            "settings.vaultDelete",
            "settings.vaultDeleteTitle",
            "settings.vaultDeleteDescription",
            "settings.vaultDeleteConfirm",
            "settings.vaultAddTitle",
            "settings.vaultName",
            "settings.vaultValue",
            "settings.vaultAdd",
            "settings.vaultNameHint",
            "settings.vaultNamePlaceholder",
            "settings.vaultValuePlaceholder",
            "settings.vaultPassphraseRequired",
            "settings.vaultMismatch",
            "settings.vaultNoVault",
            "settings.vaultLockedError",
            "settings.vaultWrongKey",
            "settings.vaultIo",
            "settings.vaultCorrupt",
            "settings.vaultEntryRequired",
            "settings.vaultEntryMissing",
            "settings.vaultLock",
            "settings.vaultLockedNotice",
            "settings.vaultAdded",
            "settings.vaultRemoved",
        ] {
            assert_ne!(tr(Lang::En, key), key, "{key} missing from en");
            assert_ne!(tr(Lang::Zh, key), key, "{key} missing from zh");
        }
        assert_eq!(
            tr(Lang::En, "settings.vaultEntryCount").replace("{count}", "2"),
            "2 entries",
            "the entry count is the pane's own template"
        );
    }

    /// The MCP pane's whole vocabulary resolves in both locales, including the
    /// connected count template the row builds.
    #[test]
    fn mcp_pane_keys_resolve_in_both_locales() {
        for key in [
            "settings.mcp",
            "settings.mcpIntro",
            "settings.mcpSearch",
            "settings.mcpStatus",
            "settings.mcpStatusConnected",
            "settings.mcpStatusFailed",
            "settings.mcpStatusPending",
            "settings.mcpStatusOff",
            "settings.mcpStatusIdle",
            "settings.mcpToolCount",
            "settings.mcpToolCountOne",
            "settings.mcpEmpty",
            "settings.mcpAdd",
            "settings.mcpEdit",
            "settings.mcpDelete",
            "settings.mcpEnable",
            "settings.mcpDisable",
            "settings.mcpDisabled",
            "settings.mcpNotConnected",
            "settings.mcpFailed",
            "settings.mcpConnected",
            "settings.mcpName",
            "settings.mcpTransport",
            "settings.mcpStdio",
            "settings.mcpHttp",
            "settings.mcpCommand",
            "settings.mcpArgs",
            "settings.mcpUrl",
            "settings.mcpScope",
            "settings.mcpScopeGlobal",
            "settings.mcpScopeWorkspace",
            "settings.mcpSave",
            "settings.mcpNameEmpty",
            "settings.mcpNameInvalid",
            "settings.mcpNameTaken",
            "settings.mcpNeedsCommand",
            "settings.mcpNeedsUrl",
            "settings.mcpUrlScheme",
            "settings.mcpMissing",
            "settings.mcpConnectFailed",
            "settings.mcpConnectedOne",
            "settings.mcpTools",
            "settings.mcpNoTools",
            "settings.mcpSaveToConnect",
        ] {
            assert_ne!(tr(Lang::En, key), key, "{key} missing from en");
            assert_ne!(tr(Lang::Zh, key), key, "{key} missing from zh");
        }
        assert_eq!(
            tr(Lang::En, "settings.mcpConnected").replace("{count}", "3"),
            "Connected · 3 tools",
            "the row's count is the pane's own template"
        );
        assert_eq!(
            tr(Lang::En, "settings.mcpConnectFailed").replace("{names}", "github"),
            "Could not connect MCP servers: github",
            "the failure notice names the servers"
        );
    }

    /// The provider's vault chooser vocabulary resolves in both locales,
    /// including the two templates that carry an entry id.
    #[test]
    fn vault_choice_keys_resolve_in_both_locales() {
        for key in [
            "settings.vaultChoose",
            "settings.vaultSource",
            "settings.vaultClear",
            "settings.vaultNoEntries",
            "settings.vaultUnlockToChoose",
            "settings.vaultCreateToChoose",
            "settings.vaultKeyFromVault",
            "settings.vaultNamedEntryMissing",
        ] {
            assert_ne!(tr(Lang::En, key), key, "{key} missing from en");
            assert_ne!(tr(Lang::Zh, key), key, "{key} missing from zh");
        }
        assert_eq!(
            tr(Lang::En, "settings.vaultSource").replace("{name}", "provider:work:api_key"),
            "vault: provider:work:api_key",
            "the source control names the chosen entry"
        );
        assert_eq!(
            tr(Lang::En, "settings.vaultNamedEntryMissing").replace("{name}", "lost-entry"),
            "The vault holds no entry named lost-entry.",
            "a named miss names the id"
        );
    }
}
