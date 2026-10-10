//! Per-session skill activation and skill authoring validation.
//!
//! [`SkillActivation`](crate::skills::session::SkillActivation) records which skills are switched on in which session, so
//! activation in one session never leaks into another. [`SkillAuthoringRecord`](crate::skills::session::SkillAuthoringRecord)
//! validates a skill definition (name, description, JSON schema) and reports
//! every problem with the name of the offending field.

use std::collections::{HashMap, HashSet};

/// Tracks which skills are active in which sessions.
#[derive(Debug, Default, Clone)]
pub struct SkillActivation {
    by_session: HashMap<String, HashSet<String>>,
}

impl SkillActivation {
    /// Create an empty activation state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Activate `skill` in `session`; returns whether it was newly activated.
    pub fn activate(&mut self, session: &str, skill: &str) -> bool {
        self.by_session
            .entry(session.to_string())
            .or_default()
            .insert(skill.to_string())
    }

    /// Deactivate `skill` in `session`; returns whether it was active.
    pub fn deactivate(&mut self, session: &str, skill: &str) -> bool {
        self.by_session
            .get_mut(session)
            .map(|skills| skills.remove(skill))
            .unwrap_or(false)
    }

    /// Whether `skill` is active in `session`.
    pub fn is_active(&self, session: &str, skill: &str) -> bool {
        self.by_session
            .get(session)
            .map(|skills| skills.contains(skill))
            .unwrap_or(false)
    }

    /// Skills active in `session`, sorted.
    pub fn list_active(&self, session: &str) -> Vec<String> {
        let mut skills: Vec<String> = self
            .by_session
            .get(session)
            .map(|skills| skills.iter().cloned().collect())
            .unwrap_or_default();
        skills.sort();
        skills
    }

    /// Sessions with at least one active skill, sorted.
    pub fn sessions(&self) -> Vec<String> {
        let mut sessions: Vec<String> = self
            .by_session
            .iter()
            .filter(|(_, skills)| !skills.is_empty())
            .map(|(session, _)| session.clone())
            .collect();
        sessions.sort();
        sessions
    }

    /// Drop every activation recorded for `session`; returns whether any existed.
    pub fn clear_session(&mut self, session: &str) -> bool {
        self.by_session.remove(session).is_some()
    }

    /// Total number of active (session, skill) pairs.
    pub fn active_count(&self) -> usize {
        self.by_session.values().map(HashSet::len).sum()
    }
}

/// One problem found while validating a [`SkillAuthoringRecord`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillValidationError {
    /// Name of the offending field, e.g. `name` or `schema`.
    pub field: String,
    /// What is wrong with that field.
    pub message: String,
}

impl SkillValidationError {
    /// Create an error for `field` with `message`.
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SkillValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// A skill definition to be validated before it is installed.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillAuthoringRecord {
    /// Skill name; non-empty, at most 128 characters, no whitespace.
    pub name: String,
    /// One-line skill description; non-empty, at most 1024 characters.
    pub description: String,
    /// JSON schema for the skill's inputs; must be a JSON object.
    pub schema: serde_json::Value,
}

impl SkillAuthoringRecord {
    /// Longest accepted skill name.
    pub const MAX_NAME_LEN: usize = 128;
    /// Longest accepted description.
    pub const MAX_DESCRIPTION_LEN: usize = 1024;

    /// Create a record from its parts.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            schema,
        }
    }

    /// Validate the record, returning every problem found.
    pub fn validate(&self) -> Result<(), Vec<SkillValidationError>> {
        let mut errors = Vec::new();

        let name = self.name.trim();
        if name.is_empty() {
            errors.push(SkillValidationError::new("name", "must not be empty"));
        } else if name.chars().count() > Self::MAX_NAME_LEN {
            errors.push(SkillValidationError::new(
                "name",
                format!("must be at most {} characters", Self::MAX_NAME_LEN),
            ));
        } else if name.chars().any(char::is_whitespace) {
            errors.push(SkillValidationError::new(
                "name",
                "must not contain whitespace",
            ));
        }

        let description = self.description.trim();
        if description.is_empty() {
            errors.push(SkillValidationError::new(
                "description",
                "must not be empty",
            ));
        } else if description.chars().count() > Self::MAX_DESCRIPTION_LEN {
            errors.push(SkillValidationError::new(
                "description",
                format!("must be at most {} characters", Self::MAX_DESCRIPTION_LEN),
            ));
        }

        if !self.schema.is_object() {
            errors.push(SkillValidationError::new("schema", "must be a JSON object"));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn activation_does_not_leak_across_sessions() {
        let mut state = SkillActivation::new();
        assert!(state.activate("s1", "web_search"));
        assert!(!state.activate("s1", "web_search"), "idempotent");

        assert!(state.is_active("s1", "web_search"));
        assert!(!state.is_active("s2", "web_search"), "not active in s2");

        // Activating in the second session is independent.
        assert!(state.activate("s2", "web_search"));
        assert!(state.is_active("s1", "web_search"));
        assert!(state.is_active("s2", "web_search"));

        // Deactivating in s1 leaves s2 untouched.
        assert!(state.deactivate("s1", "web_search"));
        assert!(!state.is_active("s1", "web_search"));
        assert!(state.is_active("s2", "web_search"));
        assert!(!state.deactivate("s1", "web_search"), "already gone");
    }

    #[test]
    fn list_active_is_sorted_and_scoped() {
        let mut state = SkillActivation::new();
        state.activate("s1", "zebra");
        state.activate("s1", "alpha");
        state.activate("s2", "beta");

        assert_eq!(state.list_active("s1"), vec!["alpha", "zebra"]);
        assert_eq!(state.list_active("s2"), vec!["beta"]);
        assert!(state.list_active("s3").is_empty());
        assert_eq!(state.sessions(), vec!["s1", "s2"]);
        assert_eq!(state.active_count(), 3);
    }

    #[test]
    fn clear_session_removes_only_that_session() {
        let mut state = SkillActivation::new();
        state.activate("s1", "a");
        state.activate("s2", "b");

        assert!(state.clear_session("s1"));
        assert!(!state.clear_session("s1"));
        assert!(!state.is_active("s1", "a"));
        assert!(state.is_active("s2", "b"));
    }

    #[test]
    fn a_valid_record_validates() {
        let record = SkillAuthoringRecord::new(
            "web-search",
            "Search the web for a query.",
            json!({"type": "object"}),
        );
        assert!(record.validate().is_ok());
    }

    #[test]
    fn empty_name_names_the_name_field() {
        let record = SkillAuthoringRecord::new("   ", "description", json!({}));
        let errors = record.validate().expect_err("invalid");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, "name");
        assert!(errors[0].to_string().contains("name"));
    }

    #[test]
    fn whitespace_in_name_is_rejected() {
        let record = SkillAuthoringRecord::new("web search", "description", json!({}));
        let errors = record.validate().expect_err("invalid");
        assert_eq!(errors[0].field, "name");
        assert!(errors[0].message.contains("whitespace"));
    }

    #[test]
    fn overlong_name_is_rejected() {
        let name = "a".repeat(SkillAuthoringRecord::MAX_NAME_LEN + 1);
        let record = SkillAuthoringRecord::new(name, "description", json!({}));
        let errors = record.validate().expect_err("invalid");
        assert_eq!(errors[0].field, "name");
        assert!(errors[0].message.contains("at most"));
    }

    #[test]
    fn empty_description_names_the_description_field() {
        let record = SkillAuthoringRecord::new("ok-name", "", json!({}));
        let errors = record.validate().expect_err("invalid");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, "description");
        assert!(errors[0].message.contains("must not be empty"));
    }

    #[test]
    fn non_object_schema_names_the_schema_field() {
        for schema in [json!("string"), json!([1, 2]), json!(null), json!(7)] {
            let record = SkillAuthoringRecord::new("ok-name", "description", schema);
            let errors = record.validate().expect_err("invalid");
            assert_eq!(errors[0].field, "schema");
            assert!(errors[0].message.contains("JSON object"));
        }
    }

    #[test]
    fn every_problem_is_reported_together() {
        let record = SkillAuthoringRecord::new("", "", json!("nope"));
        let errors = record.validate().expect_err("invalid");
        let fields: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, vec!["name", "description", "schema"]);
    }
}
