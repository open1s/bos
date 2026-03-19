#![cfg(test)]

use crate::skills::metadata::SkillMetadata;
use crate::skills::{SkillInjector, SkillLoader};
use std::collections::HashMap;
use std::path::PathBuf;

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("skills")
}

#[test]
fn test_skill_loader_discover() {
    let mut loader = SkillLoader::new(fixtures_path());
    loader.discover().unwrap();

    let skills = loader.list();
    assert_eq!(skills.len(), 2);

    let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"code-review"));
    assert!(names.contains(&"filesystem"));
}

#[test]
fn test_skill_loader_load() {
    let mut loader = SkillLoader::new(fixtures_path());
    loader.discover().unwrap();

    let content = loader.load("code-review").unwrap();
    assert_eq!(content.metadata.name, "code-review");
    assert!(content.instructions.contains("Read all changed files"));
}

#[test]
fn test_skill_metadata_parse() {
    let mut loader = SkillLoader::new(fixtures_path());
    loader.discover().unwrap();

    let skills = loader.list();
    let code_review = skills.iter().find(|s| s.name == "code-review").unwrap();

    assert_eq!(code_review.name, "code-review");
    assert_eq!(
        code_review.description,
        "Review code for issues and improvements"
    );
    assert!(code_review.path.to_string_lossy().contains("code-review"));
}

#[test]
fn test_skill_injector_available() {
    let mut skills = HashMap::new();
    skills.insert(
        "test-skill".to_string(),
        SkillMetadata {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            path: PathBuf::from("/test/path"),
        },
    );

    let xml = SkillInjector::inject_available(&skills);
    assert!(xml.contains("<available_skills>"));
    assert!(xml.contains("<name>test-skill</name>"));
    assert!(xml.contains("<description>A test skill</description>"));
    assert!(xml.contains("</available_skills>"));
}

#[test]
fn test_skill_not_found() {
    let mut loader = SkillLoader::new(fixtures_path());
    loader.discover().unwrap();

    let result = loader.load("nonexistent");
    assert!(matches!(result, Err(super::SkillError::NotFound(_))));
}
