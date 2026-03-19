use super::metadata::SkillMetadata;
use std::collections::HashMap;

pub struct SkillInjector;

impl SkillInjector {
    pub fn inject_available(skills: &HashMap<String, SkillMetadata>) -> String {
        let mut xml = String::from("<available_skills>\n");
        for meta in skills.values() {
            xml.push_str(&format!(
                " <skill>\n  <name>{}</name>\n  <description>{}</description>\n </skill>\n",
                meta.name, meta.description
            ));
        }
        xml.push_str("</available_skills>");
        xml
    }

    pub fn inject_skill_content(name: &str, content: &str) -> String {
        format!(
            "<skill_activation name=\"{}\">\n{}\n</skill_activation>",
            name, content
        )
    }
}
