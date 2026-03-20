use super::metadata::{SkillCategory, SkillContent, SkillMetadata};

pub struct SkillInjector {
    options: InjectionOptions,
}

#[derive(Debug, Clone)]
pub struct InjectionOptions {
    pub include_references: bool,
    pub include_tags: bool,
    pub include_version: bool,
    pub format: InjectionFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionFormat {
    Compact,
    Standard,
    Verbose,
}

impl Default for InjectionOptions {
    fn default() -> Self {
        Self {
            include_references: true,
            include_tags: true,
            include_version: false,
            format: InjectionFormat::Standard,
        }
    }
}

impl InjectionOptions {
    pub fn compact() -> Self {
        Self {
            include_references: false,
            include_tags: false,
            include_version: false,
            format: InjectionFormat::Compact,
        }
    }

    pub fn verbose() -> Self {
        Self {
            include_references: true,
            include_tags: true,
            include_version: true,
            format: InjectionFormat::Verbose,
        }
    }
}

impl SkillInjector {
    pub fn new() -> Self {
        Self {
            options: InjectionOptions::default(),
        }
    }

    pub fn with_options(options: InjectionOptions) -> Self {
        Self { options }
    }

    pub fn inject_available(&self, skills: &[SkillMetadata]) -> String {
        if skills.is_empty() {
            return String::new();
        }

        let mut xml = String::from("<available_skills>\n");
        for skill in skills {
            xml.push_str(&self.format_skill_metadata(skill));
            xml.push('\n');
        }
        xml.push_str("</available_skills>");
        xml
    }

    pub fn inject_specific(&self, skills: &[SkillContent], names: &[&str]) -> String {
        let selected: Vec<&SkillContent> = skills
            .iter()
            .filter(|s| names.contains(&s.metadata.name.as_str()))
            .collect();

        if selected.is_empty() {
            return String::new();
        }

        let mut xml = String::from(
            "<!-- Skills injected by: brainos-agent SkillInjector -->\n<available_skills>\n",
        );

        for skill in selected {
            xml.push_str(&self.format_standard(skill));
            xml.push('\n');
        }

        xml.push_str("</available_skills>");
        xml
    }

    pub fn inject_by_category(&self, skills: &[SkillContent], category: SkillCategory) -> String {
        let selected: Vec<&SkillContent> = skills
            .iter()
            .filter(|s| s.metadata.category == category)
            .collect();

        if selected.is_empty() {
            return String::new();
        }

        let mut xml = format!(
            "<!-- Skills in category: {} -->\n<available_skills>\n",
            category.display_name()
        );

        for skill in selected {
            xml.push_str(&self.format_standard(skill));
            xml.push('\n');
        }

        xml.push_str("</available_skills>");
        xml
    }

    pub fn inject_by_tags(&self, skills: &[SkillContent], tags: &[&str]) -> String {
        let selected: Vec<&SkillContent> = skills
            .iter()
            .filter(|s| s.metadata.tags.iter().any(|t| tags.contains(&t.as_str())))
            .collect();

        if selected.is_empty() {
            return String::new();
        }

        let mut xml = format!(
            "<!-- Skills matching tags: {} -->\n<available_skills>\n",
            tags.join(", ")
        );

        for skill in selected {
            xml.push_str(&self.format_standard(skill));
            xml.push('\n');
        }

        xml.push_str("</available_skills>");
        xml
    }

    fn format_skill_metadata(&self, skill: &SkillMetadata) -> String {
        match self.options.format {
            InjectionFormat::Compact => self.format_compact(skill),
            InjectionFormat::Standard | InjectionFormat::Verbose => {
                self.format_standard_meta(skill)
            }
        }
    }

    fn format_compact(&self, skill: &SkillMetadata) -> String {
        format!("- **{}**: {}", skill.name, skill.description)
    }

    fn format_standard_meta(&self, skill: &SkillMetadata) -> String {
        let mut xml = format!(
            "<skill>\n <name>{}</name>\n <description>{}</description>\n",
            skill.name, skill.description
        );

        if self.options.include_tags && !skill.tags.is_empty() {
            xml.push_str(&format!(" <tags>{}</tags>\n", skill.tags.join(", ")));
        }

        if self.options.include_version {
            xml.push_str(&format!(
                " <version>{}</version>\n",
                skill.version.display()
            ));
        }

        xml.push_str("</skill>");
        xml
    }

    fn format_standard(&self, skill: &SkillContent) -> String {
        let mut xml = format!(
            "<skill>\n <name>{}</name>\n <description>{}</description>\n",
            skill.metadata.name, skill.metadata.description
        );

        if self.options.include_tags && !skill.metadata.tags.is_empty() {
            xml.push_str(&format!(
                " <tags>{}</tags>\n",
                skill.metadata.tags.join(", ")
            ));
        }

        xml.push_str(&format!(
            " <instructions><![CDATA[{}]]></instructions>\n",
            skill.instructions
        ));

        if self.options.include_references && !skill.references.is_empty() {
            xml.push_str(" <references>\n");
            for ref_file in &skill.references {
                xml.push_str(&format!(
                    " <reference name=\"{}\" path=\"{}\" />\n",
                    ref_file.name,
                    ref_file.path.display()
                ));
            }
            xml.push_str(" </references>\n");
        }

        xml.push_str("</skill>");
        xml
    }

    pub fn injected_stats(&self, xml: &str) -> InjectionStats {
        InjectionStats {
            skill_count: xml.matches("<skill>").count(),
            char_count: xml.len(),
            line_count: xml.lines().count(),
        }
    }
}

impl Default for SkillInjector {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct InjectionStats {
    pub skill_count: usize,
    pub char_count: usize,
    pub line_count: usize,
}
