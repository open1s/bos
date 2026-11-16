//! Tests for prompt rendering and memory placement.

use react::prelude::{render_template, with_memory, PromptTemplate, TemplateError};
use std::collections::BTreeMap;

#[test]
fn renders_named_placeholders() {
    let mut vars = BTreeMap::new();
    vars.insert("name", "Ada");
    vars.insert("role", "engineer");
    let template = PromptTemplate::new("You are {{name}}, a {{ role }}.");
    assert_eq!(template.render(&vars).unwrap(), "You are Ada, a engineer.");
    assert_eq!(template.template(), "You are {{name}}, a {{ role }}.");
}

#[test]
fn unknown_variable_is_an_error() {
    let vars = BTreeMap::new();
    assert_eq!(
        render_template("Hello {{who}}", &vars),
        Err(TemplateError::MissingVariable("who".to_string()))
    );
}

#[test]
fn unclosed_placeholder_is_an_error() {
    let vars = BTreeMap::new();
    assert_eq!(
        render_template("Hello {{who", &vars),
        Err(TemplateError::UnclosedPlaceholder)
    );
}

#[test]
fn placeholder_positions_memory_in_place() {
    let template = "Rules.\n\n{{memory}}\n\nAct.";
    assert_eq!(
        with_memory(template, Some("Recalled")),
        "Rules.\n\nRecalled\n\nAct."
    );
    assert_eq!(with_memory(template, None), "Rules.\n\nAct.");
}

#[test]
fn without_a_placeholder_memory_is_appended() {
    assert_eq!(
        with_memory("Rules.", Some("Recalled")),
        "Rules.\n\nRecalled"
    );
    assert_eq!(with_memory("", Some("Recalled")), "Recalled");
    assert_eq!(with_memory("Rules.", None), "Rules.");
    assert_eq!(with_memory("Rules.", Some("")), "Rules.");
}
