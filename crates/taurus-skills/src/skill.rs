//! Skill definitions and frontmatter parsing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where a skill came from. Later tiers shadow earlier ones by name, so a
/// project can override a personal skill that does not fit it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SkillTier {
    /// Ships with the harness.
    Builtin,
    /// `~/.taurus/skills`
    User,
    /// `<workspace>/.taurus/skills`
    Project,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillScript {
    /// Path relative to the skill directory.
    pub path: String,
    /// Logical interpreter name: `python3`, `node`, `bash`, `sh`, `pwsh`.
    pub interpreter: String,
    /// What the script does and what arguments it takes.
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    /// The line the model actually reads when deciding whether to open this
    /// skill, so it must describe the *situation*, not the capability.
    pub when_to_use: String,
    #[serde(default = "default_version")]
    pub version: u32,
    /// Tools this skill needs. Advisory: it documents intent and drives the
    /// UI, but does not widen the session's permissions.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<SkillScript>,
}

fn default_version() -> u32 {
    1
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub frontmatter: SkillFrontmatter,
    /// Markdown after the frontmatter. Loaded on demand, never in the catalog.
    pub body: String,
    pub dir: PathBuf,
    pub tier: SkillTier,
    /// Set when the skill cannot fully run here — typically a missing
    /// interpreter. The skill stays usable; the model is told to follow the
    /// written procedure instead of calling the script.
    pub degraded: Option<String>,
}

impl Skill {
    pub fn name(&self) -> &str {
        &self.frontmatter.name
    }

    /// The one line this skill contributes to the system prompt.
    pub fn catalog_line(&self) -> String {
        format!(
            "- {}: {}",
            self.frontmatter.name, self.frontmatter.when_to_use
        )
    }
}

/// A skill summary for the UI. Excludes the body, which can be long.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub version: u32,
    pub tier: SkillTier,
    pub scripts: Vec<SkillScript>,
    pub degraded: Option<String>,
    pub dir: String,
}

impl From<&Skill> for SkillSummary {
    fn from(skill: &Skill) -> Self {
        Self {
            name: skill.frontmatter.name.clone(),
            description: skill.frontmatter.description.clone(),
            when_to_use: skill.frontmatter.when_to_use.clone(),
            version: skill.frontmatter.version,
            tier: skill.tier,
            scripts: skill.frontmatter.scripts.clone(),
            degraded: skill.degraded.clone(),
            dir: skill.dir.display().to_string(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("{path}: missing YAML frontmatter (the file must start with a --- line)")]
    NoFrontmatter { path: String },

    #[error("{path}: frontmatter is not valid YAML: {source}")]
    BadYaml {
        path: String,
        #[source]
        source: serde_yaml_ng::Error,
    },

    #[error("{path}: {message}")]
    Invalid { path: String, message: String },

    #[error("no skill named '{0}'")]
    NotFound(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Splits `---\nyaml\n---\nbody` into its two halves.
pub fn parse_skill_md(text: &str, path: &Path) -> Result<(SkillFrontmatter, String), SkillError> {
    let display = path.display().to_string();
    // Tolerate a leading BOM and Windows line endings: skills get authored by
    // models and edited by humans on every platform.
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| SkillError::NoFrontmatter {
            path: display.clone(),
        })?;
    let (yaml, body) = rest
        .split_once("\n---\n")
        .or_else(|| rest.split_once("\n---"))
        .ok_or_else(|| SkillError::NoFrontmatter {
            path: display.clone(),
        })?;

    let frontmatter: SkillFrontmatter =
        serde_yaml_ng::from_str(yaml).map_err(|source| SkillError::BadYaml {
            path: display.clone(),
            source,
        })?;

    Ok((frontmatter, body.trim_start().to_string()))
}

/// Rules a skill must satisfy to enter the catalog.
///
/// Applied both when loading from disk and when accepting a generated
/// proposal, so a model cannot write a skill that a human could not.
pub fn validate(frontmatter: &SkillFrontmatter, path: &str) -> Result<(), SkillError> {
    let invalid = |message: String| SkillError::Invalid {
        path: path.to_string(),
        message,
    };

    if !is_kebab_case(&frontmatter.name) {
        return Err(invalid(format!(
            "name '{}' must be kebab-case (lowercase letters, digits, and hyphens)",
            frontmatter.name
        )));
    }
    if frontmatter.description.trim().is_empty() {
        return Err(invalid("description must not be empty".into()));
    }
    if frontmatter.when_to_use.trim().is_empty() {
        return Err(invalid(
            "when_to_use must not be empty; it is the only text the model sees when choosing \
             a skill"
                .into(),
        ));
    }
    if frontmatter.when_to_use.chars().count() > 200 {
        return Err(invalid(format!(
            "when_to_use is {} characters; keep it under 200 so the catalog stays cheap",
            frontmatter.when_to_use.chars().count()
        )));
    }
    for script in &frontmatter.scripts {
        if script.interpreter.trim().is_empty() {
            return Err(invalid(format!(
                "script '{}' declares no interpreter",
                script.path
            )));
        }
        if script.path.contains("..") || Path::new(&script.path).is_absolute() {
            return Err(invalid(format!(
                "script path '{}' must stay inside the skill directory",
                script.path
            )));
        }
    }
    Ok(())
}

fn is_kebab_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"---
name: pdf-extract
description: Pull text out of PDFs
when_to_use: The user provides a PDF and asks about its contents
scripts:
  - path: extract.py
    interpreter: python3
    description: Extract text
---

# Steps

1. Run the script.
"#;

    fn fm(name: &str, when: &str) -> SkillFrontmatter {
        SkillFrontmatter {
            name: name.into(),
            description: "d".into(),
            when_to_use: when.into(),
            version: 1,
            allowed_tools: vec![],
            scripts: vec![],
        }
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let (front, body) = parse_skill_md(GOOD, Path::new("SKILL.md")).unwrap();
        assert_eq!(front.name, "pdf-extract");
        assert_eq!(front.version, 1, "version defaults to 1");
        assert_eq!(front.scripts[0].interpreter, "python3");
        assert!(body.starts_with("# Steps"));
    }

    #[test]
    fn parses_crlf_and_bom() {
        let text = format!("\u{feff}{}", GOOD.replace('\n', "\r\n"));
        let (front, body) = parse_skill_md(&text, Path::new("SKILL.md")).unwrap();
        assert_eq!(front.name, "pdf-extract");
        assert!(body.contains("# Steps"));
    }

    #[test]
    fn rejects_a_file_without_frontmatter() {
        let err = parse_skill_md("# Just markdown\n", Path::new("SKILL.md")).unwrap_err();
        assert!(matches!(err, SkillError::NoFrontmatter { .. }));
    }

    #[test]
    fn rejects_invalid_yaml() {
        let err =
            parse_skill_md("---\nname: [unclosed\n---\nbody", Path::new("SKILL.md")).unwrap_err();
        assert!(matches!(err, SkillError::BadYaml { .. }));
    }

    #[test]
    fn accepts_a_valid_frontmatter() {
        assert!(validate(&fm("my-skill", "when the user asks"), "p").is_ok());
    }

    #[test]
    fn rejects_a_non_kebab_case_name() {
        for bad in ["My_Skill", "my skill", "-lead", "trail-", "MySkill"] {
            assert!(
                validate(&fm(bad, "when"), "p").is_err(),
                "'{bad}' should be rejected"
            );
        }
    }

    #[test]
    fn rejects_an_empty_when_to_use() {
        assert!(validate(&fm("ok-name", "   "), "p").is_err());
    }

    #[test]
    fn rejects_an_overlong_when_to_use() {
        assert!(validate(&fm("ok-name", &"x".repeat(201)), "p").is_err());
    }

    #[test]
    fn rejects_a_script_escaping_its_directory() {
        let mut front = fm("ok-name", "when");
        front.scripts.push(SkillScript {
            path: "../../etc/evil.sh".into(),
            interpreter: "bash".into(),
            description: String::new(),
        });
        assert!(validate(&front, "p").is_err());
    }

    #[test]
    fn rejects_a_script_with_no_interpreter() {
        let mut front = fm("ok-name", "when");
        front.scripts.push(SkillScript {
            path: "x.py".into(),
            interpreter: "  ".into(),
            description: String::new(),
        });
        assert!(validate(&front, "p").is_err());
    }
}
