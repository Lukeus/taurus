//! The system prompt.
//!
//! Written for small local models: short, concrete, and negative where it
//! matters. A 7B model given a page of hedged guidance will ignore most of it,
//! so this states the few behaviors that actually change outcomes.

use std::path::Path;

const BASE: &str = "\
You are Taurus, a coding and automation agent running on the user's own machine.

# How you work

- Use tools to find things out. Do not guess at file contents, directory \
layouts, or command output.
- Prefer `read_file`, `glob`, and `grep` over shelling out to `cat`, `find`, or \
`grep -r`; they are faster and respect the project's ignore rules.
- Read a file before you edit it. `edit_file` needs the exact current text.
- Work in small steps and check the result of each one before the next.
- When a tool returns an error, read it and fix the cause. Do not retry the \
same call unchanged.
- If the user denies an action, do not attempt it another way. Ask what they \
would prefer.

# Answering

Be brief. Skip preamble, restating the question, and summaries of what you are \
about to do. When you have done the work, say what changed and stop.
";

const SKILL_AUTHORING: &str = "\
# Writing skills down

When you work out something non-obvious that will come up again — a multi-step \
workflow, a tool's quirks, a convention specific to this project — call \
`propose_skill` to record it. The user reviews every proposal, so proposing \
costs them a glance and nothing else; keep working immediately after.

Propose a skill when the procedure took real effort to get right and would take \
the same effort next time. Do not propose one for a single command, for \
something an existing skill already covers, or for facts that only matter to \
this one task.
";

/// Builds the system prompt for a session.
pub fn build(workspace: &Path, skill_section: Option<String>, synthesis_enabled: bool) -> String {
    let mut prompt = String::from(BASE);

    prompt.push_str(&format!(
        "\n# Workspace\n\nYou are working in `{}`. Every path you read or write must be inside \
         it; attempts to reach outside are refused.\n",
        workspace.display()
    ));

    prompt.push_str(&format!(
        "\n# Platform\n\nThis machine runs {}. Write commands and paths that work here.\n",
        platform_description()
    ));

    if let Some(section) = skill_section {
        prompt.push('\n');
        prompt.push_str(&section);
    }

    if synthesis_enabled {
        prompt.push('\n');
        prompt.push_str(SKILL_AUTHORING);
    }

    prompt
}

/// Named explicitly because a local model has no way to know, and will
/// otherwise emit `ls` on Windows or backslash paths on Linux.
fn platform_description() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows (commands run through cmd.exe; use Windows path separators)"
    } else if cfg!(target_os = "macos") {
        "macOS (commands run through /bin/sh; BSD versions of coreutils)"
    } else {
        "Linux (commands run through /bin/sh; GNU coreutils)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_the_workspace_path() {
        let prompt = build(Path::new("/tmp/project"), None, false);
        assert!(prompt.contains("/tmp/project"));
    }

    #[test]
    fn names_the_platform() {
        let prompt = build(Path::new("/tmp"), None, false);
        let named = ["Windows", "macOS", "Linux"]
            .iter()
            .any(|p| prompt.contains(p));
        assert!(named, "the prompt must tell the model which OS it is on");
    }

    #[test]
    fn skill_authoring_guidance_follows_the_setting() {
        assert!(build(Path::new("/tmp"), None, true).contains("propose_skill"));
        assert!(!build(Path::new("/tmp"), None, false).contains("propose_skill"));
    }

    #[test]
    fn the_skill_catalog_is_included_when_present() {
        let prompt = build(
            Path::new("/tmp"),
            Some("# Skills\n\n- alpha: when alpha\n".into()),
            false,
        );
        assert!(prompt.contains("- alpha: when alpha"));
    }
}
