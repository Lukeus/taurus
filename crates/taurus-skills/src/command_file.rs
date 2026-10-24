//! Command files: Claude Code's `commands/*.md`, read as skills.
//!
//! Claude Code folded its commands into skills — a file at
//! `commands/review.md` and a skill at `skills/review/SKILL.md` both make
//! `/review`, and take the same frontmatter — so that is what one is here
//! too: a skill with a single file, run by name. It goes through the same
//! catalog, the same `/name` resolution and the same `load_skill`, and a skill
//! keeps a name it shares with a command, as it does in Claude Code.
//!
//! What differs is the file, so loading and rendering differ:
//!
//! - **Frontmatter is optional.** Most command files are a bare prompt. The
//!   name is the file's, and without a `description` the first line of the
//!   body stands in, as it does in Claude Code.
//! - **Arguments are positional too.** `$ARGUMENTS` is the whole line typed
//!   after the name, and `$1` to `$9` are its words, a quoted phrase counting
//!   as one. Only here: a skill's body is a procedure full of shell, and
//!   `awk '{print $1}'` in one is not a placeholder.
//! - **Shell output is asked for, not run.** In Claude Code `` !`git status` ``
//!   runs before the prompt is sent. Here it becomes an instruction to run the
//!   command with `run_command`, so it goes through the permission gate like
//!   anything else the model runs. A command file can't run a program just by
//!   being typed.
//! - **`model` isn't used.** A command runs on the conversation's model, and
//!   saying so is a warning on the command rather than a silent difference.

use std::path::Path;

use serde::Deserialize;

use crate::skill::{
    condense, deserialize_tool_list, is_kebab_case, quote_scalars, Skill, SkillError,
    SkillFrontmatter, SkillOrigin, SkillTier,
};

/// What a command file's frontmatter may say. Every field is optional.
#[derive(Default, Deserialize)]
struct CommandFrontmatter {
    #[serde(default)]
    description: Option<String>,
    #[serde(
        default,
        alias = "allowed-tools",
        deserialize_with = "deserialize_tool_list"
    )]
    allowed_tools: Vec<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, alias = "disable-model-invocation")]
    disable_model_invocation: bool,
}

/// Reads one command file as a skill named after the file.
///
/// The plugin's prefix is the catalog's to add, as it is for a skill folder.
pub fn load(path: &Path, tier: SkillTier, origin: SkillOrigin) -> Result<Skill, SkillError> {
    let display = path.display().to_string();
    let invalid = |message: String| SkillError::Invalid {
        path: display.clone(),
        message,
    };

    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();
    // Refused rather than loaded with a warning, unlike a skill's name: a
    // command is only ever reached by typing it, and `/Review_PR` isn't a
    // name the composer reads as a command at all.
    if !is_kebab_case(&name) {
        return Err(invalid(format!(
            "'{name}' can't be typed as a /command. Name the file in lowercase letters, digits \
             and hyphens, like review-pr.md"
        )));
    }

    let text = std::fs::read_to_string(path)?;
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut warnings = Vec::new();
    let (front, hint, body) = split(&text, &mut warnings);

    let body = body.trim().to_string();
    if body.is_empty() {
        return Err(invalid(
            "the command is empty, so there is nothing for it to ask".into(),
        ));
    }
    let description = front
        .description
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| first_line(&body));
    if let Some(model) = front.model.filter(|m| !m.trim().is_empty()) {
        warnings.push(format!(
            "asks for the model '{model}', which isn't used: a command runs on the \
             conversation's model"
        ));
    }

    Ok(Skill {
        frontmatter: SkillFrontmatter {
            name,
            description,
            when_to_use: None,
            version: 1,
            compatibility: None,
            disable_model_invocation: front.disable_model_invocation,
            user_invocable: true,
            allowed_tools: front.allowed_tools,
            scripts: Vec::new(),
            argument_hint: hint,
        },
        body,
        dir: path.parent().unwrap_or(path).to_path_buf(),
        tier,
        origin,
        resources: Vec::new(),
        warnings,
        degraded: None,
        plugin: None,
        command_file: Some(path.to_path_buf()),
    })
}

/// Splits a command file into its frontmatter, its argument hint, and its
/// body.
///
/// The hint is taken out of the YAML by line before the rest is parsed.
/// Claude's own examples write `argument-hint: [pr-number] [priority]`, which
/// isn't YAML at all, and one field nothing checks shouldn't cost a command
/// its description. Frontmatter that still won't parse is set aside with a
/// warning, and the body after it is the command.
fn split(text: &str, warnings: &mut Vec<String>) -> (CommandFrontmatter, Option<String>, String) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (CommandFrontmatter::default(), None, text.to_string());
    };
    let Some((yaml, body)) = rest
        .split_once("\n---\n")
        .or_else(|| rest.strip_suffix("\n---").map(|yaml| (yaml, "")))
    else {
        // No closing line: the dashes are the start of the text, not a block.
        return (CommandFrontmatter::default(), None, text.to_string());
    };

    let mut hint = None;
    let kept: Vec<&str> = yaml
        .lines()
        .filter(|line| {
            let Some(value) = line
                .strip_prefix("argument-hint:")
                .or_else(|| line.strip_prefix("argument_hint:"))
            else {
                return true;
            };
            let value = value.trim().trim_matches(['"', '\'']).trim();
            hint = (!value.is_empty()).then(|| value.to_string());
            false
        })
        .collect();
    let yaml = kept.join("\n");

    let front = serde_yaml_ng::from_str::<Option<CommandFrontmatter>>(&yaml)
        .ok()
        .or_else(|| {
            quote_scalars(&yaml).and_then(|fixed| {
                serde_yaml_ng::from_str::<Option<CommandFrontmatter>>(&fixed).ok()
            })
        });
    let front = match front {
        Some(front) => front.unwrap_or_default(),
        None => {
            warnings.push(
                "frontmatter isn't valid YAML, so it's ignored and the text after it is the \
                 command"
                    .into(),
            );
            CommandFrontmatter::default()
        }
    };
    (front, hint, body.to_string())
}

/// The first line of a body, as a description: its heading marks gone, and
/// cut to the length a trigger may be.
fn first_line(body: &str) -> String {
    let line = body
        .lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    condense(line)
}

/// A command, as the model receives it.
///
/// The file itself rather than a procedure wrapped around it: a command is a
/// prompt somebody wrote to be sent, and it reads as one.
pub(crate) fn render(skill: &Skill, file: &Path, args: &str) -> String {
    let (body, asks_for_output) = shell_output(&substitute(&skill.body, args));
    let mut out = format!("# Command: /{}\n\n", skill.name());
    if asks_for_output {
        out.push_str(
            "> This command needs the output of the commands marked in it. Run each with \
             `run_command` first, and read its output where it is marked.\n\n",
        );
    }
    out.push_str(&body);
    out.push_str(&format!("\n\nCommand file: `{}`.\n", file.display()));
    out
}

/// Fills `$ARGUMENTS` and `$1` to `$9` with what was typed after the name.
///
/// One pass, so text that came in as an argument is never itself read as a
/// placeholder. A position past the last word is empty. A command with no
/// placeholder at all still gets what was typed, under a heading, for the
/// reason [`crate::skill::substitute_arguments`] gives.
pub fn substitute(body: &str, args: &str) -> String {
    let args = args.trim();
    let words = words(args);
    let mut out = String::with_capacity(body.len() + args.len());
    let mut placed = false;
    let mut rest = body;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(tail) = after.strip_prefix("ARGUMENTS") {
            out.push_str(args);
            placed = true;
            rest = tail;
            continue;
        }
        let mut digits = after.chars();
        match (digits.next(), digits.next()) {
            (Some(n @ '1'..='9'), next) if !next.is_some_and(|c| c.is_ascii_digit()) => {
                let index = n as usize - '1' as usize;
                out.push_str(words.get(index).map_or("", String::as_str));
                placed = true;
                rest = &after[1..];
            }
            _ => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    if !placed && !args.is_empty() {
        out.push_str(&format!("\n\n## User input\n\n{args}\n"));
    }
    out
}

/// The words of an argument line, a quoted phrase counting as one word.
fn words(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in args.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(current);
    }
    out
}

/// Turns each `` !`command` `` into an instruction to run it, and says
/// whether there was one.
fn shell_output(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut found = false;
    let mut rest = text;
    while let Some(at) = rest.find("!`") {
        let after = &rest[at + 2..];
        let Some(end) = after.find('`') else {
            break;
        };
        let command = &after[..end];
        out.push_str(&rest[..at]);
        if command.trim().is_empty() {
            out.push_str(&rest[at..at + 2 + end + 1]);
        } else {
            out.push_str(&format!(
                "`{command}` (run it with `run_command`; its output goes here)"
            ));
            found = true;
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    (out, found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn command(dir: &TempDir, file: &str, text: &str) -> Result<Skill, SkillError> {
        let path = dir.path().join(file);
        std::fs::write(&path, text).unwrap();
        load(&path, SkillTier::User, SkillOrigin::PluginCommand)
    }

    #[test]
    fn a_bare_prompt_is_a_command_named_after_its_file() {
        let dir = TempDir::new().unwrap();
        let cmd = command(
            &dir,
            "review-pr.md",
            "# Review a pull request\n\nReview $ARGUMENTS.\n",
        )
        .unwrap();
        assert_eq!(cmd.name(), "review-pr");
        assert_eq!(cmd.frontmatter.description, "Review a pull request");
        assert!(cmd.frontmatter.user_invocable);
        assert!(cmd.warnings.is_empty(), "{:?}", cmd.warnings);
    }

    #[test]
    fn frontmatter_says_what_claude_codes_does() {
        let dir = TempDir::new().unwrap();
        let cmd = command(
            &dir,
            "fix-issue.md",
            "---\ndescription: Fix an issue by number\nargument-hint: [issue-number] [priority]\n\
             allowed-tools: Bash(git:*) Read\nmodel: claude-opus\ndisable-model-invocation: true\n\
             ---\n\nFix issue #$1 at priority $2.\n",
        )
        .unwrap();
        assert_eq!(cmd.frontmatter.description, "Fix an issue by number");
        assert_eq!(
            cmd.frontmatter.argument_hint.as_deref(),
            Some("[issue-number] [priority]")
        );
        assert_eq!(cmd.frontmatter.allowed_tools, ["Bash(git:*)", "Read"]);
        assert!(!cmd.model_invocable());
        assert_eq!(cmd.warnings.len(), 1, "the model is said to be unused");
        assert!(cmd.warnings[0].contains("claude-opus"));
    }

    #[test]
    fn a_file_whose_name_cant_be_typed_is_refused_with_the_fix() {
        let dir = TempDir::new().unwrap();
        let err = command(&dir, "Review_PR.md", "Review it.").unwrap_err();
        assert!(err.to_string().contains("review-pr.md"), "{err}");
    }

    #[test]
    fn an_empty_command_is_refused() {
        let dir = TempDir::new().unwrap();
        assert!(command(&dir, "empty.md", "---\ndescription: nothing\n---\n\n").is_err());
    }

    #[test]
    fn broken_frontmatter_is_set_aside_and_the_body_still_runs() {
        let dir = TempDir::new().unwrap();
        let cmd = command(&dir, "odd.md", "---\n: : :\n  - [\n---\n\nDo the thing.\n").unwrap();
        assert_eq!(cmd.body, "Do the thing.");
        assert_eq!(cmd.warnings.len(), 1);
    }

    #[test]
    fn arguments_fill_by_position_and_whole() {
        let filled = substitute(
            "Fix #$1 at $2. All: $ARGUMENTS. Gone: $3.",
            r#"42 "very high""#,
        );
        assert_eq!(
            filled,
            r#"Fix #42 at very high. All: 42 "very high". Gone: ."#
        );
    }

    #[test]
    fn a_dollar_that_is_not_a_placeholder_is_left_alone() {
        assert_eq!(
            substitute("Costs $5 or $10 or $x", ""),
            "Costs  or $10 or $x"
        );
        assert_eq!(
            substitute("Price in $", "a"),
            "Price in $\n\n## User input\n\na\n"
        );
    }

    #[test]
    fn typed_text_is_never_read_as_a_placeholder() {
        assert_eq!(substitute("Say $1", "$ARGUMENTS"), "Say $ARGUMENTS");
    }

    #[test]
    fn a_command_without_placeholders_still_gets_what_was_typed() {
        assert_eq!(
            substitute("Tidy the code.", "src/lib.rs"),
            "Tidy the code.\n\n## User input\n\nsrc/lib.rs\n"
        );
        assert_eq!(substitute("Tidy the code.", "  "), "Tidy the code.");
    }

    #[test]
    fn shell_output_is_asked_for_rather_than_run() {
        let dir = TempDir::new().unwrap();
        let cmd = command(
            &dir,
            "commit.md",
            "Status: !`git status`\nDiff: !`git diff HEAD`\n\nWrite a commit message.",
        )
        .unwrap();
        let shown = cmd.render("");
        assert!(shown.starts_with("# Command: /commit\n"), "{shown}");
        assert!(
            shown.contains("Run each with `run_command` first"),
            "{shown}"
        );
        assert!(
            shown
                .contains("Status: `git status` (run it with `run_command`; its output goes here)"),
            "{shown}"
        );
        assert!(!shown.contains("!`"), "{shown}");
    }

    #[test]
    fn a_command_with_no_shell_says_nothing_about_running_any() {
        let dir = TempDir::new().unwrap();
        let cmd = command(&dir, "plain.md", "Explain `!important` in CSS.").unwrap();
        assert!(!cmd.render("").contains("run_command"));
    }
}
