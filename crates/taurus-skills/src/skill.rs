//! Skill definitions and frontmatter parsing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How widely a skill applies. Later tiers shadow earlier ones by name, so a
/// project can override a personal skill that does not fit it.
///
/// Orthogonal to [`SkillOrigin`], which says which directory convention the
/// skill was installed under. Both tiers exist under all three.
///
/// There is no `Builtin`: nothing ships with the harness, and a variant the
/// UI had a label for but no code could ever produce read as a feature someone
/// had left half-finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SkillTier {
    /// Under the home directory — available in every workspace.
    User,
    /// Under the workspace — travels with the project.
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

/// Which convention's directory a skill was found in.
///
/// Taurus reads the two shared locations as well as its own, so a skill
/// installed by another client is usable here without being copied. The origin
/// is carried through to the UI because "where did this come from" is the first
/// question asked of a skill nobody in this project wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SkillOrigin {
    /// `.taurus/skills` — Taurus's own location.
    Taurus,
    /// `.agents/skills` — the cross-client convention from the Agent Skills
    /// specification. Skills installed here are shared with every client that
    /// follows it.
    Agents,
    /// `.claude/skills` — read for compatibility, because that is where a large
    /// number of existing skills are already installed.
    Claude,
    /// GitHub Copilot's locations: `.github/skills` in a repository and
    /// `~/.copilot/skills` for a person. The only origin whose directory name
    /// differs between the two tiers, because Copilot puts a project's skills
    /// beside its workflows rather than in a dotdir of its own — which is why
    /// anything labelling this has to know the tier as well.
    ///
    /// Read for the same reason `.claude/skills` is, and cheaply: Copilot reads
    /// the same `SKILL.md` specification, so a skill written for it is already
    /// a skill Taurus understands. Nothing here parses a second format.
    Copilot,
}

/// The longest trigger line that may reach the system prompt.
pub const TRIGGER_MAX: usize = 200;

/// Length limits from the Agent Skills specification. Exceeding either is
/// reported and then tolerated: a name three characters too long still names
/// the skill, and refusing to load it would help nobody.
pub const NAME_MAX: usize = 64;
pub const DESCRIPTION_MAX: usize = 1024;

/// How much of a `description` may stand in for a missing `when_to_use`.
///
/// Longer than [`TRIGGER_MAX`] because a spec-shaped `description` is doing two
/// jobs at once — what the skill does *and* when to use it — and cutting it to
/// 200 characters routinely severed the second half, which is the half the
/// model is actually reading.
pub const DESCRIPTION_TRIGGER_MAX: usize = 300;

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct SkillFrontmatter {
    pub name: String,
    /// What the skill does and when to use it.
    ///
    /// The only field besides `name` that the Agent Skills specification
    /// requires, so it is the only one a borrowed skill is guaranteed to carry.
    pub description: String,
    /// The line the model actually reads when deciding whether to open this
    /// skill, so it must describe the *situation*, not the capability.
    ///
    /// Optional, because it is a Taurus field and no other client writes it. A
    /// skill without one falls back to a condensed `description` — see
    /// [`Skill::trigger`]. Skills authored here still set it: 200 characters
    /// aimed at the decision beats 1024 characters aimed at a catalog listing,
    /// and on an 8k context that difference is the whole budget.
    ///
    /// Skipped when absent on the way back out, along with every other empty
    /// field: what Taurus writes here is read by other clients too, and a
    /// frontmatter full of `null` and `[]` is a worse file than a short one.
    #[serde(
        default,
        alias = "when-to-use",
        skip_serializing_if = "Option::is_none"
    )]
    pub when_to_use: Option<String>,
    #[serde(default = "default_version")]
    pub version: u32,
    /// Environment this skill expects — an interpreter version, a system
    /// package, network access.
    ///
    /// Shown next to the skill rather than acted on. Taurus cannot verify a
    /// free-text claim, but a borrowed skill saying "Requires Python 3.14+ and
    /// uv" is exactly what a reader wants before running it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    /// Whether the model may choose this skill on its own.
    ///
    /// When set, the skill is left out of the prompt catalog entirely rather
    /// than listed and refused — a skill the model can see but not open wastes
    /// a turn discovering that. It stays available as a slash command, which is
    /// the point of the flag: some procedures should run when a person asks for
    /// them and not before.
    #[serde(
        default,
        alias = "disable-model-invocation",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub disable_model_invocation: bool,
    /// Whether a person may run this skill directly as `/name`.
    ///
    /// Defaults to true, so every skill is reachable by name without its author
    /// having to say so. Skills written for other clients carry the field and
    /// mean the same thing by it.
    #[serde(
        default = "default_true",
        alias = "user-invocable",
        skip_serializing_if = "Clone::clone"
    )]
    pub user_invocable: bool,
    /// Tools this skill expects to use.
    ///
    /// Advisory, and deliberately so: it is shown next to the skill so a reader
    /// can see what it will reach for before running it, and it never widens
    /// what the session is permitted to do. A skill cannot grant itself the
    /// shell by listing `run_command` — every call still goes through the same
    /// permission gate as any other.
    ///
    /// Accepts both spellings the ecosystem uses: a YAML list, and the spec's
    /// space-separated string (`allowed-tools: Bash(git:*) Read`).
    #[serde(
        default,
        alias = "allowed-tools",
        deserialize_with = "deserialize_tool_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<SkillScript>,
}

// The specification's other optional fields — `license`, `metadata` — are
// deliberately not modelled. Unknown keys are already ignored, so they cost
// nothing today; modelling `metadata` as the spec's string-to-string map would
// instead make `version: 1.0` under it a hard parse failure, which is a way to
// reject a skill over a field nothing reads.

fn default_version() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

/// Accepts `allowed_tools` as either a YAML list or a space-separated string.
fn deserialize_tool_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ListOrString {
        List(Vec<String>),
        String(String),
    }

    Ok(match Option::<ListOrString>::deserialize(deserializer)? {
        None => Vec::new(),
        Some(ListOrString::List(list)) => list,
        Some(ListOrString::String(s)) => s.split_whitespace().map(str::to_string).collect(),
    })
}

/// Collapses a `description` into something the size of a trigger line.
///
/// Whitespace is flattened first because a folded YAML scalar arrives with the
/// author's line breaks in it, and a catalog is one line per skill.
fn condense(description: &str) -> String {
    let flat = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= DESCRIPTION_TRIGGER_MAX {
        return flat;
    }
    let cut = flat
        .char_indices()
        .nth(DESCRIPTION_TRIGGER_MAX)
        .map_or(flat.len(), |(i, _)| i);
    let head = &flat[..cut];
    // Back off to a word boundary: half a word reads as corruption, not as a
    // truncation the reader can mentally complete.
    let head = head.rfind(' ').map_or(head, |i| &head[..i]);
    format!("{}…", head.trim_end_matches([',', ';', ':', '.', ' ']))
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub frontmatter: SkillFrontmatter,
    /// Markdown after the frontmatter. Loaded on demand, never in the catalog.
    pub body: String,
    pub dir: PathBuf,
    pub tier: SkillTier,
    pub origin: SkillOrigin,
    /// Files bundled beside `SKILL.md` — `scripts/`, `references/`, `assets/`.
    ///
    /// Listed at load time and named to the model when the skill is opened, but
    /// never read: the whole point of the convention is that a reference file
    /// costs nothing until the procedure actually calls for it.
    pub resources: Vec<String>,
    /// Things wrong with the skill that did not stop it loading.
    ///
    /// Kept on the skill rather than in the load-problem list, because a skill
    /// that works is not a failure — it belongs on its own row in the drawer,
    /// not under "could not load".
    pub warnings: Vec<String>,
    /// Set when the skill cannot fully run here — typically a missing
    /// interpreter. The skill stays usable; the model is told to follow the
    /// written procedure instead of calling the script.
    pub degraded: Option<String>,
}

impl Skill {
    pub fn name(&self) -> &str {
        &self.frontmatter.name
    }

    /// The text the model reads when deciding whether this skill applies.
    ///
    /// `when_to_use` when the author wrote one, and a condensed `description`
    /// otherwise — which is every skill borrowed from another client, since
    /// `when_to_use` is a Taurus field.
    pub fn trigger(&self) -> String {
        match &self.frontmatter.when_to_use {
            Some(when) if !when.trim().is_empty() => when.clone(),
            _ => condense(&self.frontmatter.description),
        }
    }

    /// Whether the model may pick this skill out of the catalog itself.
    pub fn model_invocable(&self) -> bool {
        !self.frontmatter.disable_model_invocation
    }

    /// The one line this skill contributes to the system prompt.
    pub fn catalog_line(&self) -> String {
        format!("- {}: {}", self.frontmatter.name, self.trigger())
    }

    /// The absolute path of one bundled resource.
    ///
    /// Resources are held as logical paths joined with `/`, which is what the
    /// scripts listing shows and what `starts_with("scripts/")` tests against.
    /// Joining one of those onto a directory whole would hand Windows
    /// `C:\skills\alpha\references/REFERENCE.md` — a path that happens to open,
    /// but written in two separators at once, in a line whose whole purpose is
    /// to be copied into another tool call.
    pub fn resource_path(&self, resource: &str) -> PathBuf {
        let mut path = self.dir.clone();
        path.extend(resource.split('/'));
        path
    }

    /// The full skill, as the model receives it.
    ///
    /// One rendering for both ways in — the `load_skill` tool and a slash
    /// command — because they deliver the same thing and the model should not
    /// have to recognize two shapes for it. `args` is what the user typed after
    /// the command, and is empty for a tool call.
    pub fn render(&self, args: &str) -> String {
        let mut out = format!(
            "# Skill: {}\n\n{}\n\n",
            self.frontmatter.name, self.frontmatter.description
        );

        if let Some(reason) = &self.degraded {
            // Surfaced prominently: silently letting the model call a script
            // that cannot run wastes a whole round trip on a confusing error.
            out.push_str(&format!(
                "> This skill's scripts cannot run on this machine ({reason}). Follow the written \
                 steps manually instead of calling run_skill_script.\n\n"
            ));
        } else if !self.frontmatter.scripts.is_empty() {
            out.push_str("## Scripts\n\nCall these with `run_skill_script`:\n\n");
            for script in &self.frontmatter.scripts {
                out.push_str(&format!(
                    "- `{}` ({}) — {}\n",
                    script.path, script.interpreter, script.description
                ));
            }
            out.push('\n');
        }

        out.push_str("## Procedure\n\n");
        out.push_str(&substitute_arguments(&self.body, args));

        // Named, never read. A skill's reference files are the third tier of
        // progressive disclosure: listing them costs a line each and lets the
        // model fetch the one it needs, while reading them here would put every
        // file of every opened skill into the context to no purpose.
        //
        // Absolute, because the model resolves these against the workspace
        // otherwise, and a skill under the home directory is not there.
        out.push_str(&format!(
            "\n\nSkill directory: `{}`. Paths in this procedure are relative to it; pass them to \
             read_file with that prefix.\n",
            self.dir.display()
        ));
        let listable: Vec<&String> = self
            .resources
            .iter()
            .filter(|r| !r.starts_with("scripts/"))
            .collect();
        if !listable.is_empty() {
            out.push_str("\nBundled files, to read only if the procedure calls for one:\n\n");
            for resource in listable {
                out.push_str(&format!("- `{}`\n", self.resource_path(resource).display()));
            }
        }
        out
    }
}

/// Fills a skill's `$ARGUMENTS` placeholder with what the user typed.
///
/// The placeholder is the convention slash-command skills are written around,
/// so honoring it is most of what makes a borrowed one work here rather than
/// leaving the model to read the word `$ARGUMENTS` and guess.
///
/// A skill that has no placeholder still gets the text, appended under a
/// heading: dropping what the user typed because the author never wrote
/// `$ARGUMENTS` would lose the request itself.
pub fn substitute_arguments(body: &str, args: &str) -> String {
    let args = args.trim();
    if body.contains(ARGUMENTS_PLACEHOLDER) {
        return body.replace(ARGUMENTS_PLACEHOLDER, args);
    }
    if args.is_empty() {
        return body.to_string();
    }
    format!("{body}\n\n## User input\n\n{args}\n")
}

const ARGUMENTS_PLACEHOLDER: &str = "$ARGUMENTS";

/// A skill summary for the UI. Excludes the body, which can be long.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    /// Already resolved through [`Skill::trigger`], so the UI shows the same
    /// text the model sees rather than an empty field for every borrowed skill.
    pub when_to_use: String,
    pub version: u32,
    pub tier: SkillTier,
    pub origin: SkillOrigin,
    /// Environment the skill says it needs. Shown, never enforced.
    pub compatibility: Option<String>,
    /// Advisory. Shown so a reader can see what the skill reaches for; it
    /// grants nothing.
    pub allowed_tools: Vec<String>,
    pub scripts: Vec<SkillScript>,
    /// Wrong but survivable. Shown on the skill's own row, not with the things
    /// that failed to load.
    pub warnings: Vec<String>,
    pub degraded: Option<String>,
    pub dir: String,
}

impl From<&Skill> for SkillSummary {
    fn from(skill: &Skill) -> Self {
        Self {
            name: skill.frontmatter.name.clone(),
            description: skill.frontmatter.description.clone(),
            when_to_use: skill.trigger(),
            version: skill.frontmatter.version,
            tier: skill.tier,
            origin: skill.origin,
            compatibility: skill.frontmatter.compatibility.clone(),
            allowed_tools: skill.frontmatter.allowed_tools.clone(),
            scripts: skill.frontmatter.scripts.clone(),
            warnings: skill.warnings.clone(),
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

/// A parsed `SKILL.md`, plus anything about it worth telling the user.
///
/// Warnings are carried rather than returned separately because they belong to
/// the skill: they are shown on its row, not in a list of things that failed.
#[derive(Debug)]
pub struct Parsed {
    pub frontmatter: SkillFrontmatter,
    pub body: String,
    pub warnings: Vec<String>,
}

/// Splits `---\nyaml\n---\nbody` into its two halves.
pub fn parse_skill_md(text: &str, path: &Path) -> Result<Parsed, SkillError> {
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

    let mut warnings = Vec::new();
    let frontmatter: SkillFrontmatter = match serde_yaml_ng::from_str(yaml) {
        Ok(frontmatter) => frontmatter,
        // One retry with the unquoted values quoted. Skills authored for other
        // clients routinely contain `description: Use when: the user …`, which
        // is invalid YAML that a laxer parser happens to accept — and rejecting
        // a skill over a colon is a bad trade when the fix is unambiguous.
        Err(original) => match quote_scalars(yaml)
            .and_then(|fixed| serde_yaml_ng::from_str::<SkillFrontmatter>(&fixed).ok())
        {
            Some(frontmatter) => {
                warnings.push(
                    "frontmatter is not valid YAML — a value contains an unquoted colon. Read as \
                     if quoted; quote it to be sure it is read the same everywhere."
                        .to_string(),
                );
                frontmatter
            }
            None => {
                return Err(SkillError::BadYaml {
                    path: display,
                    source: original,
                })
            }
        },
    };

    Ok(Parsed {
        frontmatter,
        body: body.trim_start().to_string(),
        warnings,
    })
}

/// Quotes top-level scalar values that contain a colon.
///
/// Deliberately narrow: only unindented `key: value` lines whose value is a
/// plain scalar. Anything indented, nested, already quoted, or opening a block
/// scalar or collection is left exactly as written, because this runs on a file
/// that already failed to parse and a broader rewrite would be guessing at
/// structure rather than repairing a known mistake.
///
/// Returns `None` when there was nothing to change, so the caller reports the
/// original parse error instead of an identical second one.
fn quote_scalars(yaml: &str) -> Option<String> {
    let mut changed = false;
    let fixed = yaml
        .lines()
        .map(|line| {
            let Some((key, value)) = line.split_once(": ") else {
                return line.to_string();
            };
            let value = value.trim();
            let plain_key = !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            let quotable = !value.is_empty()
                && !value.starts_with(['"', '\'', '|', '>', '[', '{', '&', '*', '#', '!'])
                && value.contains(':');
            if plain_key && quotable {
                changed = true;
                format!("{key}: '{}'", value.replace('\'', "''"))
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    changed.then_some(fixed)
}

/// Rules a skill must satisfy to enter the catalog.
///
/// Applied both when loading from disk and when accepting a generated
/// proposal, so a model cannot write a skill that a human could not.
///
/// The split between a warning and an error is the difference between a skill
/// that is wrong and one that cannot work. A name in the wrong case still
/// identifies the skill; a missing description leaves nothing to choose it by.
/// Warnings are returned rather than logged so the caller can decide — loading
/// from disk shows them and carries on, while a generated proposal treats any
/// of them as a rejection, since nothing should be written here that only
/// loads by leniency.
pub fn validate(frontmatter: &SkillFrontmatter, path: &str) -> Result<Vec<String>, SkillError> {
    let invalid = |message: String| SkillError::Invalid {
        path: path.to_string(),
        message,
    };
    let mut warnings = Vec::new();

    if !is_kebab_case(&frontmatter.name) {
        warnings.push(format!(
            "name '{}' is not kebab-case (lowercase letters, digits, and hyphens)",
            frontmatter.name
        ));
    }
    if frontmatter.name.chars().count() > NAME_MAX {
        warnings.push(format!(
            "name is {} characters; the format allows {NAME_MAX}",
            frontmatter.name.chars().count()
        ));
    }
    // Required by the specification, and the fallback trigger when a skill
    // carries no `when_to_use`. Without it there is nothing to put in the
    // catalog, so the skill could never be chosen.
    if frontmatter.description.trim().is_empty() {
        return Err(invalid(
            "description must not be empty; it is what the model reads when choosing a skill"
                .into(),
        ));
    }
    if frontmatter.description.chars().count() > DESCRIPTION_MAX {
        warnings.push(format!(
            "description is {} characters; the format allows {DESCRIPTION_MAX}",
            frontmatter.description.chars().count()
        ));
    }
    // Only checked when present. It is a Taurus field, and a skill written for
    // another client will not have one.
    if let Some(when) = &frontmatter.when_to_use {
        if when.trim().is_empty() {
            return Err(invalid(
                "when_to_use is present but empty; remove it to fall back to the description, or \
                 say when the skill applies"
                    .into(),
            ));
        }
        if when.chars().count() > TRIGGER_MAX {
            return Err(invalid(format!(
                "when_to_use is {} characters; keep it under {TRIGGER_MAX} so the catalog stays \
                 cheap",
                when.chars().count()
            )));
        }
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
    Ok(warnings)
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

    /// A spec-minimal skill: the two required fields and nothing else. This is
    /// the shape every skill borrowed from another client arrives in.
    const SPEC_MINIMAL: &str = r#"---
name: pdf-processing
description: Extract PDF text, fill forms, merge files. Use when handling PDFs.
license: Apache-2.0
compatibility: Requires Python 3.14+ and uv
metadata:
  author: example-org
  version: "1.0"
allowed-tools: Bash(git:*) Bash(jq:*) Read
---

Do the thing.
"#;

    fn fm(name: &str, when: &str) -> SkillFrontmatter {
        SkillFrontmatter {
            name: name.into(),
            description: "d".into(),
            when_to_use: Some(when.into()),
            version: 1,
            compatibility: None,
            disable_model_invocation: false,
            user_invocable: true,
            allowed_tools: vec![],
            scripts: vec![],
        }
    }

    fn skill(front: SkillFrontmatter) -> Skill {
        Skill {
            frontmatter: front,
            body: String::new(),
            dir: PathBuf::from("/tmp/skill"),
            tier: SkillTier::User,
            origin: SkillOrigin::Taurus,
            resources: Vec::new(),
            warnings: Vec::new(),
            degraded: None,
        }
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let Parsed {
            frontmatter: front,
            body,
            ..
        } = parse_skill_md(GOOD, Path::new("SKILL.md")).unwrap();
        assert_eq!(front.name, "pdf-extract");
        assert_eq!(front.version, 1, "version defaults to 1");
        assert_eq!(front.scripts[0].interpreter, "python3");
        assert!(body.starts_with("# Steps"));
    }

    #[test]
    fn parses_crlf_and_bom() {
        let text = format!("\u{feff}{}", GOOD.replace('\n', "\r\n"));
        let Parsed {
            frontmatter: front,
            body,
            ..
        } = parse_skill_md(&text, Path::new("SKILL.md")).unwrap();
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
    fn rejects_an_empty_when_to_use() {
        assert!(validate(&fm("ok-name", "   "), "p").is_err());
    }

    #[test]
    fn parses_a_spec_minimal_skill_from_another_client() {
        let Parsed {
            frontmatter: front,
            body,
            ..
        } = parse_skill_md(SPEC_MINIMAL, Path::new("SKILL.md")).unwrap();
        assert_eq!(front.name, "pdf-processing");
        assert!(
            front.when_to_use.is_none(),
            "no client but Taurus writes it"
        );
        assert_eq!(
            front.compatibility.as_deref(),
            Some("Requires Python 3.14+ and uv")
        );
        // `license` and `metadata` are unmodelled; a nested map under one of
        // them must not take the skill down with it.
        assert_eq!(body.trim(), "Do the thing.");
        assert!(validate(&front, "SKILL.md").is_ok());
    }

    #[test]
    fn reads_allowed_tools_as_a_space_separated_string_or_a_list() {
        let front = parse_skill_md(SPEC_MINIMAL, Path::new("SKILL.md"))
            .unwrap()
            .frontmatter;
        assert_eq!(front.allowed_tools, ["Bash(git:*)", "Bash(jq:*)", "Read"]);

        let listed = "---\nname: n\ndescription: d\nallowed_tools: [read_file, grep]\n---\nbody";
        let front = parse_skill_md(listed, Path::new("SKILL.md"))
            .unwrap()
            .frontmatter;
        assert_eq!(front.allowed_tools, ["read_file", "grep"]);
    }

    #[test]
    fn a_missing_when_to_use_falls_back_to_the_description() {
        let front = parse_skill_md(SPEC_MINIMAL, Path::new("SKILL.md"))
            .unwrap()
            .frontmatter;
        assert_eq!(
            skill(front).trigger(),
            "Extract PDF text, fill forms, merge files. Use when handling PDFs."
        );
    }

    #[test]
    fn when_to_use_wins_over_the_description() {
        let mut front = fm("n", "when the user asks");
        front.description = "something else entirely".into();
        assert_eq!(skill(front).trigger(), "when the user asks");
    }

    #[test]
    fn a_long_description_is_condensed_to_one_line() {
        let mut front = fm("n", "w");
        front.when_to_use = None;
        // Folded across lines, as YAML block scalars arrive, and far past the
        // cap so both behaviours are exercised at once.
        front.description = format!("first line\nsecond line {}", "verbose ".repeat(60));

        let trigger = skill(front).trigger();
        assert!(!trigger.contains('\n'), "must be one catalog line");
        assert!(trigger.starts_with("first line second line verbose"));
        assert!(trigger.ends_with('…'));
        assert!(
            trigger.chars().count() <= DESCRIPTION_TRIGGER_MAX + 1,
            "{} characters is over the cap",
            trigger.chars().count()
        );
    }

    #[test]
    fn a_short_description_is_left_alone() {
        let mut front = fm("n", "w");
        front.when_to_use = None;
        front.description = "Short and complete.".into();
        assert_eq!(skill(front).trigger(), "Short and complete.");
    }

    #[test]
    fn reads_a_value_with_an_unquoted_colon() {
        // Invalid YAML that other clients' parsers accept, and the single most
        // common way a real skill file fails to load here.
        let text = "---\nname: pdf\ndescription: Use this skill when: the user asks about PDFs\n\
                    ---\nbody";
        let parsed = parse_skill_md(text, Path::new("SKILL.md")).unwrap();
        assert_eq!(
            parsed.frontmatter.description,
            "Use this skill when: the user asks about PDFs"
        );
        assert_eq!(
            parsed.warnings.len(),
            1,
            "the repair is reported, not silent"
        );
        assert!(parsed.warnings[0].contains("unquoted colon"));
    }

    #[test]
    fn the_colon_repair_leaves_valid_files_untouched() {
        let parsed = parse_skill_md(GOOD, Path::new("SKILL.md")).unwrap();
        assert!(parsed.warnings.is_empty());
        // Nested keys under `scripts:` must survive: the repair is line-wise
        // and could have quoted its way through a list of maps.
        assert_eq!(parsed.frontmatter.scripts[0].path, "extract.py");
    }

    #[test]
    fn yaml_that_no_quoting_can_fix_is_still_an_error() {
        let err =
            parse_skill_md("---\nname: [unclosed\n---\nbody", Path::new("SKILL.md")).unwrap_err();
        assert!(matches!(err, SkillError::BadYaml { .. }));
    }

    #[test]
    fn a_name_in_the_wrong_shape_warns_rather_than_fails() {
        // The specification's rules, relaxed on load: a skill someone already
        // installed is more useful read than refused.
        for bad in ["My_Skill", "my skill", "-lead", "trail-"] {
            let warnings = validate(&fm(bad, "when"), "p")
                .unwrap_or_else(|e| panic!("'{bad}' should load with a warning, got {e}"));
            assert_eq!(warnings.len(), 1, "'{bad}'");
        }
    }

    #[test]
    fn an_overlong_name_or_description_warns() {
        let long_name = "a".repeat(NAME_MAX + 1);
        assert_eq!(validate(&fm(&long_name, "when"), "p").unwrap().len(), 1);

        let mut front = fm("ok-name", "when");
        front.description = "d".repeat(DESCRIPTION_MAX + 1);
        assert_eq!(validate(&front, "p").unwrap().len(), 1);
    }

    #[test]
    fn a_well_formed_skill_warns_about_nothing() {
        assert!(validate(&fm("my-skill", "when the user asks"), "p")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_an_empty_description() {
        let mut front = fm("ok-name", "when");
        front.description = "  ".into();
        assert!(validate(&front, "p").is_err());
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
