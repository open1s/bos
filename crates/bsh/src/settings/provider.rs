//! The provider tree as an editing surface.
//!
//! The tree *is* the document: `GlobalSettings` holds the providers, their
//! connections and their models, and this module holds every operation the
//! panes and forms perform on them. Nothing here keeps a second copy of a
//! provider, so there is no parallel model to drift out of step with the file,
//! and no leaf can be lost on the way through a round trip.
//!
//! Two rules decide where a selection may land:
//!
//! - A connection is provider level. A model names a *leaf key* under the
//!   provider whose connection it uses, which is what the reference editor
//!   calls inherited: a model carries no endpoint of its own.
//! - A provider's protocol is the family default, and a leaf overrides it.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use super::global::{self, GlobalSettings, ModelEntry, Provider};

/// The wire protocols a provider can speak, as the value the leaf's api_mode
/// and the engine's AgentConfig::api_mode carry.
///
/// Anthropic Messages only reaches this shell through the anthropic vendor,
/// whose clients send it unconditionally; the OpenAI transports refuse the
/// protocol rather than guessing.
pub(crate) const PROTOCOLS: [&str; 3] = ["chat", "responses", "anthropic"];

/// The protocol a provider speaks when neither the family nor the user says.
pub(crate) const DEFAULT_PROTOCOL: &str = "chat";

/// A provider with no models of its own.
static NO_MODELS: BTreeMap<String, ModelEntry> = BTreeMap::new();

/// Why a provider or a model was refused.
///
/// Each variant is a sentence in the locale files; the pane shows the sentence
/// and the form keeps every field the user typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderRefusal {
    /// A required provider name (or, at the store's own guard, provider id)
    /// was left blank.
    NameEmpty,
    /// A provider needs a base URL.
    NeedsBaseUrl,
    /// The base URL is not an HTTP or HTTPS address.
    BaseUrlInvalid,
    /// No model id was given.
    NeedsModel,
    /// A provider already carries that id.
    IdTaken,
    /// No model id was given in the model row.
    ModelIdRequired,
    /// Two models of one provider share an id.
    ModelIdDuplicate,
    /// A display name was written but left blank.
    ModelNameInvalid,
}

impl ProviderRefusal {
    /// The locale key of the sentence this refusal draws.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::NameEmpty => "settings.providerNameEmpty",
            Self::NeedsBaseUrl => "settings.providerNeedsBaseUrl",
            Self::BaseUrlInvalid => "settings.models.customBaseUrlInvalid",
            Self::NeedsModel => "settings.providerNeedsModel",
            Self::IdTaken => "settings.providerIdTaken",
            Self::ModelIdRequired => "settings.modelIdRequired",
            Self::ModelIdDuplicate => "settings.models.modelIdDuplicate",
            Self::ModelNameInvalid => "settings.models.modelNameInvalid",
        }
    }
}

/// The protocol a provider id speaks when its family declares one.
pub(crate) fn family_protocol(provider: &str) -> &'static str {
    match global::built_in(provider) {
        Some((_, _, protocol)) => protocol,
        None => DEFAULT_PROTOCOL,
    }
}

/// The display name a provider id ships with, when it is a shipped family.
pub(crate) fn family_name(provider: &str) -> Option<&'static str> {
    global::built_in(provider).map(|(name, _, _)| name)
}

/// The base URL a provider id ships with, when it is a shipped family.
pub(crate) fn family_base_url(provider: &str) -> Option<&'static str> {
    global::built_in(provider).map(|(_, base_url, _)| base_url)
}

/// The first model of a provider that declares text and nothing else.
///
/// The shipped families send images on every model, so declaring one
/// text-only is the user narrowing what the composer offers it. The pane says
/// so on the row rather than refusing the declaration.
pub(crate) fn text_only(provider: &Provider) -> Option<&str> {
    provider.models.values().find_map(|leaf| {
        let declared = leaf.input_types.as_deref()?;
        if declared.iter().any(|kind| kind == "image") {
            return None;
        }
        Some(leaf.model.as_str())
    })
}

/// Whether a leaf answers image input.
///
/// Undeclared is permissive: a model the user never classified keeps taking
/// whatever the shell sends it, which is also what a leaf written before this
/// field existed must keep doing.
pub(crate) fn accepts_image(leaf: &ModelEntry) -> bool {
    !is_text_only(leaf)
}

/// Whether a leaf declares text and nothing else.
pub(crate) fn is_text_only(leaf: &ModelEntry) -> bool {
    matches!(
        leaf.input_types.as_deref(),
        Some(declared) if !declared.iter().any(|kind| kind == "image")
    )
}

/// Whether a leaf shows one input type as declared.
pub(crate) fn shows_input_type(leaf: &ModelEntry, kind: &str) -> bool {
    match leaf.input_types.as_deref() {
        None | Some([]) => true,
        Some(declared) => declared.iter().any(|entry| entry == kind),
    }
}

/// Switch one input type on a leaf.
///
/// The first toggle materializes the declaration, because a leaf that declares
/// nothing shows both types lit. Text is never dropped: a model that takes no
/// text cannot answer a prompt at all.
pub(crate) fn toggle_input_type(leaf: &mut ModelEntry, kind: &str) {
    let declared: Vec<String> = match leaf.input_types.as_deref() {
        None | Some([]) => vec!["text".to_string(), "image".to_string()],
        Some(current) => current.to_vec(),
    };
    let mut next: Vec<String> = declared
        .iter()
        .filter(|entry| entry.as_str() != kind)
        .cloned()
        .collect();
    if next.len() == declared.len() {
        next.push(kind.to_string());
    }
    if !next.iter().any(|entry| entry == "text") {
        next.insert(0, "text".to_string());
    }
    leaf.input_types = Some(next);
}

/// The refusal a base URL draws, if any.
pub(crate) fn base_url_refusal(raw: &str) -> Option<ProviderRefusal> {
    let value = raw.trim();
    if value.is_empty() {
        return Some(ProviderRefusal::NeedsBaseUrl);
    }
    if !(value.starts_with("http://") || value.starts_with("https://")) {
        return Some(ProviderRefusal::BaseUrlInvalid);
    }
    None
}

/// Every input type the pane offers, in the order it draws them.
///
/// This is the vocabulary, not a declaration: a leaf that says nothing about
/// its inputs takes all of them, which is why an absent list reads as
/// permissive rather than as text only.
fn all_input_types() -> &'static [&'static str] {
    &["text", "image"]
}
fn model_id_refusal(leaf: &ModelEntry, taken: &BTreeSet<String>) -> Option<ProviderRefusal> {
    let id = leaf.model.trim();
    if id.is_empty() {
        return Some(ProviderRefusal::ModelIdRequired);
    }
    // A composed id comes from a registry whose keys are folded, so a repeat is
    // judged in lower case; a flat id is what the reader typed and compares
    // exactly. The fold is a fact about the id, never about what inputs a leaf
    // declares, so the two answers cannot lean on each other.
    let fold = |value: &str| {
        if value.contains("/") {
            value.to_ascii_lowercase()
        } else {
            value.to_string()
        }
    };
    let folded = fold(id);
    if taken.iter().any(|other| fold(other) == folded) {
        return Some(ProviderRefusal::ModelIdDuplicate);
    }
    if let Some(name) = leaf.name.as_deref() {
        if name.trim().is_empty() {
            return Some(ProviderRefusal::ModelNameInvalid);
        }
    }
    None
}

/// A display name as the form typed it.
///
/// Unlike [`display_name`] this keeps a blank, because a blank is a refusal and
/// not an absence: the pane has to see the difference between a field the user
/// cleared and one they never filled in.
fn typed_name(name: &str) -> Option<String> {
    (!name.is_empty()).then(|| name.to_string())
}

/// Keep the fields the first leaf already set, and fill the rest from a second
/// leaf that shares its key.
///
/// An id is unique within its provider, so the second leaf cannot be a
/// separate model. Merging rather than overwriting keeps what the file held:
/// the field that produced the key never displaces a value a user wrote.
fn keep_first(entry: &mut ModelEntry, arrival: &ModelEntry) {
    if entry.name.is_none() {
        entry.name = arrival.name.clone();
    }
    if entry.max_tokens.is_none() {
        entry.max_tokens = arrival.max_tokens;
    }
    if entry.api_mode.is_none() {
        entry.api_mode = arrival.api_mode.clone();
    }
    if entry.reasoning_effort.is_none() {
        entry.reasoning_effort = arrival.reasoning_effort.clone();
    }
    if entry.input_types.is_none() {
        entry.input_types = arrival.input_types.clone();
    }
}

/// A display name that is worth writing, which an empty one and the id are
/// not.
fn display_name(name: &str, id: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && name != id).then(|| name.to_string())
}

impl GlobalSettings {
    /// The models one provider offers, keyed by nickname.
    ///
    /// Reading a provider that is not declared gives an empty map rather than
    /// nothing, so a caller can ask for its models before it exists.
    pub(crate) fn models_of(&self, provider: &str) -> &BTreeMap<String, ModelEntry> {
        self.providers
            .get(provider)
            .map(|entry| &entry.models)
            .unwrap_or(&NO_MODELS)
    }

    /// One model of one provider, by nickname.
    pub(crate) fn model_entry(&self, provider: &str, nickname: &str) -> Option<&ModelEntry> {
        self.providers.get(provider)?.models.get(nickname)
    }

    /// One model of one provider, by nickname, for writing.
    pub(crate) fn model_entry_mut(
        &mut self,
        provider: &str,
        nickname: &str,
    ) -> Option<&mut ModelEntry> {
        self.providers.get_mut(provider)?.models.get_mut(nickname)
    }

    /// The provider and model a workspace reference resolves to.
    pub(crate) fn find_model(
        &self,
        provider: &str,
        nickname: &str,
    ) -> Option<(&Provider, &ModelEntry)> {
        let entry = self.providers.get(provider)?;
        Some((entry, entry.models.get(nickname)?))
    }

    /// Every model id a provider carries, trimmed for comparison.
    ///
    /// The fold happens in [`model_id_refusal`], where the family decides
    /// whether its ids compare exactly or in lower case.
    pub(crate) fn model_ids(&self, provider: &str) -> BTreeSet<String> {
        self.models_of(provider)
            .values()
            .map(|leaf| leaf.model.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect()
    }

    /// Every model id a provider carries under a nickname other than this one.
    pub(crate) fn other_model_ids(&self, provider: &str, nickname: &str) -> BTreeSet<String> {
        self.models_of(provider)
            .iter()
            .filter(|(key, _)| key.as_str() != nickname)
            .map(|(_, leaf)| leaf.model.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect()
    }

    /// Every model a provider carries, as nickname to model id.
    pub(crate) fn model_refs(&self, provider: &str) -> BTreeMap<String, String> {
        self.models_of(provider)
            .iter()
            .map(|(nickname, leaf)| (nickname.clone(), leaf.model.trim().to_string()))
            .collect()
    }

    /// The protocol a provider id speaks when no leaf overrides it.
    ///
    /// The node carries the choice the pane made, which a shipped family is
    /// born with; so a protocol set on the provider reaches every model under
    /// it that names none of its own.
    pub(crate) fn family_protocol_of(&self, provider: &str) -> String {
        self.providers
            .get(provider)
            .and_then(|node| node.api_mode.clone())
            .filter(|mode| !mode.trim().is_empty())
            .unwrap_or_else(|| family_protocol(provider).to_string())
    }

    /// The protocol a model is called with.
    pub(crate) fn protocol(&self, provider: &str, nickname: &str) -> String {
        match self.model_entry(provider, nickname) {
            Some(leaf) => leaf
                .api_mode
                .as_deref()
                .map(str::trim)
                .filter(|mode| !mode.is_empty())
                .map(|mode| mode.to_string())
                .unwrap_or_else(|| self.family_protocol_of(provider)),
            None => self.family_protocol_of(provider),
        }
    }

    /// The model id a provider declares text-only, if any.
    pub(crate) fn text_only_model(&self, provider: &str) -> Option<&str> {
        self.providers.get(provider).and_then(text_only)
    }

    /// Whether one model of one provider answers image input.
    pub(crate) fn accepts_image(&self, provider: &str, nickname: &str) -> bool {
        match self.model_entry(provider, nickname) {
            Some(leaf) => accepts_image(leaf),
            None => true,
        }
    }

    /// Whether one model of one provider shows an input type as declared.
    pub(crate) fn shows_input_type(&self, provider: &str, nickname: &str, kind: &str) -> bool {
        match self.model_entry(provider, nickname) {
            Some(leaf) => shows_input_type(leaf, kind),
            None => true,
        }
    }

    /// Whether a preset id is served by the shipped list rather than declared.
    ///
    /// A declaration with the same id replaces the shipped row, so this is
    /// false once one exists.
    pub(crate) fn preset_is_shipped(&self, id: &str) -> bool {
        global::ships_preset(id) && !self.presets.contains_key(id)
    }

    /// Add a provider that speaks a shipped family protocol.
    ///
    /// The id keys the tree and is the prefix the engine registers the vendor
    /// under, so it stays fixed. The display name is written as typed unless it
    /// is blank or repeats the id, which is what a row falls back to anyway.
    /// The connection is the provider's: a model never carries one. The models
    /// named here land under their own ids as nicknames.
    pub(crate) fn add_provider(
        &mut self,
        id: &str,
        name: &str,
        base_url: &str,
        models: &[ModelEntry],
    ) {
        let id = id.trim();
        if id.is_empty() || self.providers.contains_key(id) {
            return;
        }
        self.providers.insert(
            id.to_string(),
            Provider {
                name: display_name(name, id),
                base_url: base_url.trim().to_string(),
                api_key_vault: None,
                legacy_api_key: None,
                declared: false,
                api_mode: Some(family_protocol(id).to_string()),
                models: BTreeMap::new(),
            },
        );
        for leaf in models {
            let _ = self.add_named_model(id, "", leaf.clone());
        }
    }

    /// Add a provider the shipped families do not list.
    pub(crate) fn add_custom_provider(
        &mut self,
        id: &str,
        name: &str,
        base_url: &str,
        models: &[ModelEntry],
    ) {
        let id = id.trim();
        if id.is_empty() || self.providers.contains_key(id) {
            return;
        }
        self.providers.insert(
            id.to_string(),
            Provider {
                name: display_name(name, id),
                base_url: base_url.trim().to_string(),
                api_key_vault: None,
                legacy_api_key: None,
                declared: true,
                api_mode: None,
                models: BTreeMap::new(),
            },
        );
        for leaf in models {
            let _ = self.add_named_model(id, "", leaf.clone());
        }
    }

    /// Drop a provider and everything under it.
    pub(crate) fn remove_provider(&mut self, id: &str) -> bool {
        self.providers.remove(id).is_some()
    }

    /// Point a provider at another connection.
    pub(crate) fn set_connection(&mut self, id: &str, base_url: &str) {
        if let Some(provider) = self.providers.get_mut(id) {
            provider.base_url = base_url.trim().to_string();
        }
    }

    /// Name the vault entry a provider reads its API key from.
    ///
    /// An empty id clears the choice rather than writing `Some("")`, so the
    /// provider falls back to the conventional `provider:<id>:api_key` entry.
    pub(crate) fn set_api_key_vault(&mut self, id: &str, entry: Option<&str>) {
        let entry = entry
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        if let Some(provider) = self.providers.get_mut(id) {
            provider.api_key_vault = entry;
        }
    }

    /// Set a provider's display name, writing nothing when it is the id.
    pub(crate) fn set_provider_name(&mut self, id: &str, name: &str) {
        if let Some(provider) = self.providers.get_mut(id) {
            provider.name = display_name(name, id);
        }
    }

    /// Land one model, keyed by the nickname the caller chose.
    ///
    /// This is the whole of what a form does when it saves: the catalogue line
    /// first, then whatever the user typed, extending the leaf it lands on.
    /// A blank nickname means the model id is its own nickname.
    pub(crate) fn add_model(
        &mut self,
        provider: &str,
        nickname: &str,
        display: &str,
        model: &str,
        api_mode: &str,
    ) -> Result<(), ProviderRefusal> {
        let leaf = ModelEntry {
            model: model.trim().to_string(),
            name: typed_name(display),
            ..ModelEntry::default()
        };
        let key = if nickname.trim().is_empty() {
            leaf.model.clone()
        } else {
            nickname.trim().to_string()
        };
        if self.add_named_model(provider, nickname, leaf)? {
            let mode = api_mode.trim();
            if !mode.is_empty() {
                if let Some(entry) = self.model_entry_mut(provider, &key) {
                    entry.api_mode = Some(mode.to_string());
                }
            }
        }
        Ok(())
    }

    /// Land one model on a provider, without a selection's protocol.
    ///
    /// Answers whether this call is what put the leaf there.
    fn add_named_model(
        &mut self,
        provider: &str,
        nickname: &str,
        mut leaf: ModelEntry,
    ) -> Result<bool, ProviderRefusal> {
        let id = leaf.model.trim();
        if id.is_empty() {
            return Err(ProviderRefusal::ModelIdRequired);
        }
        leaf.model = id.to_string();
        // A fresh leaf declares nothing, and an absent declaration reads as
        // permissive, so it keeps sending images until the pane says otherwise.
        let key = match nickname.trim() {
            "" => leaf.model.clone(),
            nickname => nickname.to_string(),
        };
        let taken = match self.model_entry(provider, &key) {
            // A model already under this nickname is extended, not repeated:
            // the merge below keeps the field that produced the key.
            Some(_) => self.other_model_ids(provider, &key),
            None => self.model_ids(provider),
        };
        if let Some(refusal) = model_id_refusal(&leaf, &taken) {
            return Err(refusal);
        }
        if let Some(entry) = self.model_entry_mut(provider, &key) {
            let mut arrival = leaf;
            arrival.name = arrival.name.filter(|name| !name.trim().is_empty());
            keep_first(entry, &arrival);
            return Ok(false);
        }
        if leaf
            .name
            .as_deref()
            .map(str::trim)
            .is_some_and(str::is_empty)
        {
            leaf.name = None;
        }
        self.providers
            .entry(provider.to_string())
            .or_default()
            .models
            .insert(key, leaf);
        Ok(true)
    }

    /// Replace a model's leaf with an edited one.
    pub(crate) fn update_model(
        &mut self,
        provider: &str,
        nickname: &str,
        leaf: &ModelEntry,
    ) -> Result<(), ProviderRefusal> {
        let mut next = leaf.clone();
        let taken = self.other_model_ids(provider, nickname);
        if let Some(refusal) = model_id_refusal(&next, &taken) {
            return Err(refusal);
        }
        if next
            .name
            .as_deref()
            .map(str::trim)
            .is_some_and(str::is_empty)
        {
            next.name = None;
        }
        if let Some(entry) = self.model_entry_mut(provider, nickname) {
            *entry = next;
        }
        Ok(())
    }

    /// Drop one model.
    pub(crate) fn remove_model(&mut self, provider: &str, nickname: &str) -> bool {
        self.providers
            .get_mut(provider)
            .map(|entry| entry.models.remove(nickname).is_some())
            .unwrap_or(false)
    }

    /// Switch one input type on one model.
    pub(crate) fn set_input_type(&mut self, provider: &str, nickname: &str, kind: &str) {
        if let Some(entry) = self.model_entry_mut(provider, nickname) {
            toggle_input_type(entry, kind);
        }
    }

    /// The providers carrying one model id, in id order.
    ///
    /// A model id repeats across a gateway and a direct connection, so the
    /// window names every connection that reaches it rather than picking one.
    pub(crate) fn providers_carrying(&self, model: &str) -> Vec<String> {
        let model = model.trim();
        if model.is_empty() {
            return Vec::new();
        }
        self.providers
            .iter()
            .filter(|(_, provider)| {
                provider
                    .models
                    .values()
                    .any(|leaf| leaf.model.trim() == model)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }
    /// The shipped provider ids the file does not carry yet, in id order.
    ///
    /// This is the catalogue the add card offers: an id the tree already
    /// holds has been taken, so it is not offered again.
    pub(crate) fn addable_providers(&self) -> Vec<String> {
        global::BUILT_IN_PROVIDERS
            .iter()
            .map(|(id, ..)| id.to_string())
            .filter(|id| !self.providers.contains_key(id))
            .collect()
    }

    /// The protocol every model of a provider family speaks, unless one leaf
    /// overrides it.
    ///
    /// The family is what a newly added model inherits, so changing it here
    /// leaves existing leaves alone.
    pub(crate) fn set_family_protocol(&mut self, provider: &str, protocol: &str) {
        if let Some(node) = self.providers.get_mut(provider) {
            node.api_mode = Some(protocol.trim().to_string());
        }
    }

    /// The input types a provider declares for one model.
    ///
    /// A missing declaration means the model takes everything, which the
    /// types a family declares expand to.
    pub(crate) fn input_types_of(&self, provider: &str, nickname: &str) -> Vec<String> {
        all_input_types()
            .iter()
            .filter(|kind| self.shows_input_type(provider, nickname, kind))
            .map(|kind| kind.to_string())
            .collect()
    }
}

impl ModelEntry {
    /// A leaf that carries only the model id.
    pub(crate) fn named(model: &str) -> Self {
        Self {
            model: model.trim().to_string(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::global::Preset;
    use super::*;

    /// A tree with one shipped family whose model id is also its nickname.
    fn with_model() -> GlobalSettings {
        let mut global = GlobalSettings::default();
        global.add_provider(
            "nvidia",
            "NVIDIA",
            "https://integrate.api.nvidia.com/v1",
            &[ModelEntry::named("nvidia/moonshotai/kimi-k3")],
        );
        global
    }

    #[test]
    fn a_family_protocol_lands_a_connection_and_its_model() {
        let global = with_model();
        let provider = global.providers.get("nvidia").expect("the provider");
        assert_eq!(provider.base_url, "https://integrate.api.nvidia.com/v1");
        assert_eq!(provider.api_key_vault, None);
        assert_eq!(provider.name.as_deref(), Some("NVIDIA"));
        assert_eq!(
            global.model_refs("nvidia"),
            BTreeMap::from([(
                "nvidia/moonshotai/kimi-k3".to_string(),
                "nvidia/moonshotai/kimi-k3".to_string()
            )])
        );
    }

    #[test]
    fn a_family_that_compares_in_lower_case_declares_nothing() {
        let mut global = with_model();
        let id = "nvidia/moonshotai/kimi-k3";
        assert_eq!(
            global.text_only_model("nvidia"),
            None,
            "a fresh leaf declares no input type"
        );
        assert!(
            global.accepts_image("nvidia", id),
            "absent reads as permissive, so the leaf keeps sending images"
        );
        global.set_input_type("nvidia", id, "image");
        assert!(
            !global.accepts_image("nvidia", id),
            "the pane said otherwise"
        );
        let refusal = global.add_model("nvidia", "again", "", &id.to_uppercase(), "");
        assert_eq!(
            refusal,
            Err(ProviderRefusal::ModelIdDuplicate),
            "a composed id is judged in lower case, so the case-folded repeat is caught"
        );
    }

    #[test]
    fn a_custom_provider_speaks_chat_and_keeps_its_name() {
        let mut global = GlobalSettings::default();
        global.add_custom_provider("gateway", "Gateway", "https://gw.example/v1", &[]);
        let provider = global.providers.get("gateway").expect("the provider");
        assert_eq!(provider.name.as_deref(), Some("Gateway"));
        assert!(provider.declared);
        assert_eq!(global.family_protocol_of("gateway"), "chat");
        assert_eq!(global.protocol("gateway", "missing"), "chat");
    }

    #[test]
    fn a_second_model_on_one_gateway_reuses_the_connection() {
        let mut global = GlobalSettings::default();
        global.add_custom_provider("gw", "gw", "https://gw.example/v1", &[]);
        global
            .add_model("gw", "fast", "", "org/fast", "responses")
            .expect("the first model");
        global
            .add_model("gw", "slow", "Slow", "org/slow", "chat")
            .expect("the second model");
        let provider = global.providers.get("gw").expect("the provider");
        assert_eq!(provider.base_url, "https://gw.example/v1");
        assert_eq!(provider.models.len(), 2);
        assert_eq!(global.protocol("gw", "fast"), "responses");
        assert_eq!(global.protocol("gw", "slow"), "chat");
        let slow = global.model_entry("gw", "slow").expect("the second model");
        assert_eq!(slow.name.as_deref(), Some("Slow"));
    }

    #[test]
    fn a_duplicate_model_id_keeps_the_first_and_extends_it() {
        let mut global = with_model();
        let first = "nvidia/moonshotai/kimi-k3";
        global
            .add_model("nvidia", first, "", first, "chat")
            .expect("the same id again");
        assert_eq!(global.models_of("nvidia").len(), 1);
        assert_eq!(global.protocol("nvidia", first), "chat");
        assert_eq!(
            global.model_entry("nvidia", first).expect("the leaf").model,
            first
        );
    }

    #[test]
    fn a_model_without_an_id_is_refused() {
        let mut global = with_model();
        let refusal = global
            .add_model("nvidia", "blank", "", "   ", "")
            .expect_err("a blank id");
        assert_eq!(refusal, ProviderRefusal::ModelIdRequired);
        assert_eq!(refusal.key(), "settings.modelIdRequired");
        assert_eq!(global.models_of("nvidia").len(), 1);
    }

    #[test]
    fn a_second_nickname_may_not_repeat_an_id() {
        let mut global = with_model();
        let first = "nvidia/moonshotai/kimi-k3";
        let refusal = global
            .add_model("nvidia", "again", "", first, "")
            .expect_err("the id is taken");
        assert_eq!(refusal, ProviderRefusal::ModelIdDuplicate);
        assert_eq!(refusal.key(), "settings.models.modelIdDuplicate");
    }

    #[test]
    fn an_edited_model_keeps_its_own_id_and_the_others_reject_it() {
        let mut global = with_model();
        global
            .add_model("nvidia", "fast", "", "org/fast", "")
            .expect("a second model");
        let mut leaf = global
            .model_entry("nvidia", "fast")
            .expect("the leaf")
            .clone();
        leaf.model = "nvidia/moonshotai/kimi-k3".to_string();
        let refusal = global
            .update_model("nvidia", "fast", &leaf)
            .expect_err("the id belongs to another model");
        assert_eq!(refusal, ProviderRefusal::ModelIdDuplicate);
        leaf.model = "org/faster".to_string();
        global
            .update_model("nvidia", "fast", &leaf)
            .expect("its own id is fine");
        assert_eq!(global.protocol("nvidia", "fast"), "chat");
    }

    #[test]
    fn a_blank_display_name_is_refused_and_a_missing_one_is_not() {
        let mut global = with_model();
        let mut leaf = ModelEntry::named("org/one");
        leaf.name = Some("   ".to_string());
        let refusal = global
            .add_model("nvidia", "one", "   ", "org/one", "")
            .expect_err("a blank name is written");
        assert_eq!(refusal.key(), "settings.models.modelNameInvalid");
        assert!(global.model_entry("nvidia", "one").is_none());
        leaf.name = None;
        global
            .update_model("nvidia", "nvidia/moonshotai/kimi-k3", &leaf)
            .expect("an absent name is fine");
        assert_eq!(
            global
                .model_entry("nvidia", "nvidia/moonshotai/kimi-k3")
                .expect("the leaf")
                .name,
            None
        );
    }

    #[test]
    fn the_last_input_type_cannot_be_switched_off() {
        let mut global = with_model();
        let name = "nvidia/moonshotai/kimi-k3";
        // Undeclared reads as permissive, so the pane lights both boxes.
        assert!(global.shows_input_type("nvidia", name, "image"));
        assert!(global.shows_input_type("nvidia", name, "text"));

        // Switching Image off declares text only, and images stop.
        global.set_input_type("nvidia", name, "image");
        assert!(!global.accepts_image("nvidia", name), "image is refused");
        assert!(
            global.shows_input_type("nvidia", name, "text"),
            "text stays"
        );

        // Switching Image back on restores the permissive answer.
        global.set_input_type("nvidia", name, "image");
        assert!(global.accepts_image("nvidia", name), "image is back");
        assert_eq!(
            global.input_types_of("nvidia", name),
            ["text".to_string(), "image".to_string()]
        );

        // Switching Text off keeps it: text is the one type that cannot go.
        global.set_input_type("nvidia", name, "text");
        assert!(
            global.shows_input_type("nvidia", name, "text"),
            "text survives"
        );
        assert!(
            global.accepts_image("nvidia", name),
            "and image stays with it"
        );
    }

    #[test]
    fn a_removed_model_leaves_the_connection_alone() {
        let mut global = with_model();
        assert!(global.remove_model("nvidia", "nvidia/moonshotai/kimi-k3"));
        assert!(global.models_of("nvidia").is_empty());
        assert_eq!(
            global
                .providers
                .get("nvidia")
                .expect("the provider")
                .base_url,
            "https://integrate.api.nvidia.com/v1"
        );
        assert!(!global.remove_model("nvidia", "nvidia/moonshotai/kimi-k3"));
    }

    #[test]
    fn the_protocol_falls_back_to_the_family() {
        let mut global = GlobalSettings::default();
        global.add_provider(
            "deepseek",
            "DeepSeek",
            "https://api.deepseek.com",
            &[ModelEntry::named("deepseek-chat")],
        );
        assert_eq!(global.family_protocol_of("deepseek"), "responses");
        assert_eq!(global.protocol("deepseek", "deepseek-chat"), "responses");
        global
            .add_model("deepseek", "other", "", "deepseek-reasoner", "chat")
            .expect("a second model");
        assert_eq!(
            global.protocol("deepseek", "deepseek-chat"),
            "responses",
            "a sibling's override does not move this leaf"
        );
        assert_eq!(global.protocol("deepseek", "other"), "chat");
    }

    #[test]
    fn a_base_url_must_be_an_address() {
        assert_eq!(base_url_refusal("   "), Some(ProviderRefusal::NeedsBaseUrl));
        assert_eq!(
            base_url_refusal("api.example.com"),
            Some(ProviderRefusal::BaseUrlInvalid)
        );
        assert_eq!(base_url_refusal(" https://api.example.com/v1 "), None);
        assert_eq!(
            ProviderRefusal::BaseUrlInvalid.key(),
            "settings.models.customBaseUrlInvalid"
        );
    }

    #[test]
    fn a_provider_named_by_the_pane_writes_only_a_name_it_changed() {
        let mut global = with_model();
        global.set_provider_name("nvidia", "My NVIDIA");
        assert_eq!(
            global.providers.get("nvidia").expect("the provider").name,
            Some("My NVIDIA".to_string())
        );
        global.set_provider_name("nvidia", "nvidia");
        assert_eq!(
            global.providers.get("nvidia").expect("the provider").name,
            None,
            "the id is the palette fallback, not a name to write"
        );
    }

    #[test]
    fn the_connections_carrying_a_model_are_listed() {
        let mut global = with_model();
        global.add_custom_provider("direct", "direct", "https://example.com/v1", &[]);
        global
            .add_model("direct", "glm", "", "nvidia/moonshotai/kimi-k3", "chat")
            .expect("the same id elsewhere");
        assert_eq!(
            global.providers_carrying("nvidia/moonshotai/kimi-k3"),
            vec!["direct".to_string(), "nvidia".to_string()]
        );
        assert!(global.providers_carrying("  ").is_empty());
    }

    #[test]
    fn the_tree_round_trips_with_a_quoted_nickname() {
        let mut global = with_model();
        global
            .add_model(
                "nvidia",
                "glm-5.3-flash",
                "",
                "zai-org/glm-5.3-flash",
                "chat",
            )
            .expect("a model whose nickname carries a dot");
        let text = global.to_toml().expect("the file");
        let back = GlobalSettings::from_toml(&text).expect("it reparses");
        assert_eq!(back, global);
        assert_eq!(
            back.model_entry("nvidia", "glm-5.3-flash")
                .expect("the dotted nickname")
                .model,
            "zai-org/glm-5.3-flash"
        );
    }

    #[test]
    fn the_shipped_preset_is_served_and_a_declaration_replaces_it() {
        let mut global = GlobalSettings::default();
        assert!(global.preset_is_shipped("standard"));
        assert_eq!(
            global.preset("standard").expect("the preset").plugins,
            vec!["shell".to_string(), "files".to_string(), "plan".to_string()]
        );
        assert!(global.preset("no-such-preset").is_none());
        global.presets.insert(
            "standard".to_string(),
            Preset {
                plugins: vec!["shell".to_string()],
                ..Preset::default()
            },
        );
        assert!(!global.preset_is_shipped("standard"));
        assert_eq!(
            global.preset("standard").expect("the declaration").plugins,
            vec!["shell".to_string()]
        );
    }
}
