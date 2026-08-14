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

# Running commands

- Every command starts in the workspace root. You are already inside the \
project — do not go looking for it, and do not `cd` anywhere before working.
- Write paths relative to that root: `cargo test`, `ls src`, `python \
scripts/build.py`. Do not paste absolute paths into a command.
- To work in a subdirectory, pass `cwd` rather than putting `cd` in the command.

# Finishing what you started

- Keep going until the task is actually done. Do not stop after one step to \
report progress, to describe what you are about to do next, or to ask whether \
to carry on — you are expected to carry on.
- Check your own work before you call it finished. If the project has tests, \
run them. If you edited a file, the tool result already told you it applied; \
what you have not verified is whether the result builds and behaves.
- If something blocks you and no tool can get past it, stop and say what \
blocked you. That is the one good reason to finish early.

# Answering

Be brief. Skip preamble, restating the question, and summaries of what you are \
about to do. When the work is done, say what changed and stop.
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

/// Guidance for `propose_agent`, on the same rule as [`SKILL_AUTHORING`]: the
/// tool is only advertised when its setting is on, so this only appears then.
///
/// The distinction it draws is the one a model gets wrong. Both a skill and an
/// agent capture something learned; the difference is whether you follow it
/// yourself or hand it to someone else, and a roster of delegates that should
/// have been procedures costs a line of every request for work you could have
/// done inline.
const AGENT_AUTHORING: &str = "\
# Writing sub-agents down

When a task has a shape that recurs and a scope worth narrowing — reviewing a \
diff, auditing one kind of file, working through a migration site by site — \
call `propose_agent` to record it as a delegate. The user reviews every \
proposal, so proposing costs them a glance and nothing else; keep working \
immediately after. The agent is not available in this turn even if approved.

Propose an agent when the work is better handed over than followed: it needs \
its own context, its own tool scope, or a narrower brief than the conversation \
you are in. Propose a skill instead when the answer is a procedure *you* should \
follow next time. Do not propose an agent that duplicates one on the roster, \
and do not propose one for the task in front of you.
";

/// Builds the system prompt for a session.
pub fn build(
    workspace: &Path,
    skill_section: Option<String>,
    synthesis_enabled: bool,
    agent_synthesis_enabled: bool,
) -> String {
    let mut prompt = String::from(BASE);

    // The path is named because a command's output will mention it and the
    // model should recognize it. Naming it alone was not enough: given an
    // absolute path and nothing else, a model builds absolute commands out of
    // it, wanders into the parent directory looking for the project it is
    // already standing in, and spends the whole iteration budget there. So the
    // path comes with the only two facts that make it actionable — you are
    // already in it, and relative is how you say so.
    prompt.push_str(&format!(
        "\n# Workspace\n\nYou are working in `{}`, and every tool call and command already starts \
         there. Refer to files by paths relative to it — `src/main.rs`, not the full path. \
         Anything outside it is refused.\n",
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

    if agent_synthesis_enabled {
        prompt.push('\n');
        prompt.push_str(AGENT_AUTHORING);
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
        let prompt = build(Path::new("/tmp/project"), None, false, false);
        assert!(prompt.contains("/tmp/project"));
    }

    #[test]
    fn says_that_commands_already_start_in_the_workspace() {
        // The fix for a model that goes hunting for the project it is already
        // standing in: given only an absolute path, it built absolute commands
        // out of it, `cd`ed into the parent, and spent an entire turn there.
        let prompt = build(Path::new("/tmp/project"), None, false, false);
        assert!(prompt.contains("already start"), "{prompt}");
        assert!(prompt.contains("relative"), "{prompt}");
    }

    #[test]
    fn tells_the_model_to_finish_and_check_its_work() {
        let prompt = build(Path::new("/tmp/project"), None, false, false);
        assert!(
            prompt.contains("until the task is actually done"),
            "{prompt}"
        );
        assert!(prompt.contains("run them"), "{prompt}");
    }

    #[test]
    fn names_the_platform() {
        let prompt = build(Path::new("/tmp"), None, false, false);
        let named = ["Windows", "macOS", "Linux"]
            .iter()
            .any(|p| prompt.contains(p));
        assert!(named, "the prompt must tell the model which OS it is on");
    }

    #[test]
    fn skill_authoring_guidance_follows_the_setting() {
        assert!(build(Path::new("/tmp"), None, true, false).contains("propose_skill"));
        assert!(!build(Path::new("/tmp"), None, false, false).contains("propose_skill"));
    }

    #[test]
    fn the_skill_catalog_is_included_when_present() {
        let prompt = build(
            Path::new("/tmp"),
            Some("# Skills\n\n- alpha: when alpha\n".into()),
            false,
            false,
        );
        assert!(prompt.contains("- alpha: when alpha"));
    }

    #[test]
    fn agent_authoring_guidance_follows_its_own_setting() {
        // Two settings, two sections. Turning skills off must not silently take
        // the agent guidance with it, or the model is offered `propose_agent`
        // with nothing telling it when to reach for one.
        assert!(build(Path::new("/tmp"), None, false, true).contains("propose_agent"));
        assert!(!build(Path::new("/tmp"), None, true, false).contains("propose_agent"));
    }

    #[test]
    fn the_two_authoring_sections_say_how_they_differ() {
        // The distinction a model gets wrong. An agent that should have been a
        // skill costs a roster line on every request for work it could have
        // done inline.
        let prompt = build(Path::new("/tmp"), None, true, true);
        assert!(prompt.contains("Propose a skill instead"), "{prompt}");
    }
}
