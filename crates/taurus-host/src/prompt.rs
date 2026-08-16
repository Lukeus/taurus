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

# Keeping track on a long task

- When a request needs more than two or three steps, call `update_plan` with \
the whole list before you start the first one.
- Call it again every time a step starts and every time one finishes. The plan \
is repeated back to you before every step, so it is how you know where you are.
- Exactly one step is `active` at a time. Finish it before starting the next.
- The plan is on the user's screen. Do not restate it in your reply, and do not \
ask them to approve it — write it and get to work.

# Answering

Be brief. Skip preamble, restating the question, and summaries of what you are \
about to do. When the work is done, say what changed and stop.

# Showing your answer

These tools draw into the conversation instead of returning text to you. What \
they draw is already on screen, so never repeat its contents in your reply.

- `show_table` — several rows of comparable facts, where the comparison is the \
point. The reader can sort and copy it.
- `show_chart` — a series whose shape is the point: where the spike is, whether \
a number is climbing.
- `ask_user` — a decision that is genuinely the user's to make.

Use them sparingly. Prose is the default, a markdown table is fine for two \
columns, and a chart of three bars is slower to read than the sentence it \
replaced. Say what the table or chart shows in your own words as well: it is \
the evidence, not the answer.

`ask_user` is the one exception to carrying on without stopping. Ask only when \
the readings of the request lead to different work and picking wrong would \
waste most of it. Do not ask to confirm a plan, to report progress, to ask \
whether to continue, or about anything you could settle by reading the code. \
Ask before you start, not partway through. Every question can be skipped, so \
be ready to decide anyway and say what you picked.
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
    instructions_section: Option<String>,
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

    // Ahead of the skill catalog, and directly after the rules it may contradict.
    // A user's standing instructions are the one part of this prompt that can
    // disagree with the part above — "ask before you touch the database" against
    // "keep going until the task is done" — and a small model resolves a
    // contradiction by recency. So the instructions come second, where they win.
    // The catalog and the authoring sections that follow are reference material
    // rather than rival rules, so nothing is lost by putting them after.
    if let Some(section) = instructions_section {
        prompt.push('\n');
        prompt.push_str(&section);
    }

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
        let prompt = build(Path::new("/tmp/project"), None, None, false, false);
        assert!(prompt.contains("/tmp/project"));
    }

    #[test]
    fn says_that_commands_already_start_in_the_workspace() {
        // The fix for a model that goes hunting for the project it is already
        // standing in: given only an absolute path, it built absolute commands
        // out of it, `cd`ed into the parent, and spent an entire turn there.
        let prompt = build(Path::new("/tmp/project"), None, None, false, false);
        assert!(prompt.contains("already start"), "{prompt}");
        assert!(prompt.contains("relative"), "{prompt}");
    }

    #[test]
    fn tells_the_model_to_finish_and_check_its_work() {
        let prompt = build(Path::new("/tmp/project"), None, None, false, false);
        assert!(
            prompt.contains("until the task is actually done"),
            "{prompt}"
        );
        assert!(prompt.contains("run them"), "{prompt}");
    }

    #[test]
    fn the_one_exception_to_carrying_on_is_stated_next_to_the_rule_it_breaks() {
        // "Do not stop to ask whether to carry on" and "ask_user" are a
        // contradiction to a 7B model unless the prompt draws the line itself.
        // Both sections are unconditional, so this holds for every session.
        let prompt = build(Path::new("/tmp"), None, None, false, false);
        assert!(prompt.contains("Do not stop after one step"), "{prompt}");
        assert!(prompt.contains("one exception"), "{prompt}");
        assert!(prompt.contains("Do not ask to confirm a plan"), "{prompt}");
    }

    #[test]
    fn the_drawing_tools_are_told_not_to_be_repeated_in_prose() {
        // The failure this prevents: a table drawn, then every row of it
        // written out again underneath, which is worse than either alone.
        let prompt = build(Path::new("/tmp"), None, None, false, false);
        assert!(prompt.contains("never repeat its contents"), "{prompt}");
    }

    #[test]
    fn the_model_is_told_when_to_plan_and_when_to_update_it() {
        // The tool existing is not enough: on a three-step task a real model
        // did the work correctly and never reached for it, because nothing in
        // the prompt said to. The threshold and the update rule are both here
        // because either alone produces a plan that is written once and then
        // goes stale — which is worse than none, since it is read back as
        // current on every iteration.
        let prompt = build(Path::new("/tmp"), None, None, false, false);
        assert!(prompt.contains("more than two or three steps"), "{prompt}");
        assert!(prompt.contains("every time a step starts"), "{prompt}");
        assert!(prompt.contains("one step is `active`"), "{prompt}");
    }

    #[test]
    fn the_plan_is_not_offered_up_for_approval() {
        // It sits beside "do not stop to ask whether to carry on", and a model
        // handed a planning tool reads it as an invitation to check in. The
        // prompt says both things in the same breath for that reason.
        let prompt = build(Path::new("/tmp"), None, None, false, false);
        assert!(prompt.contains("do not ask them to approve it"), "{prompt}");
        // Said twice on purpose, from both sides: the planning section, and the
        // rule about what `ask_user` is not for.
        assert!(prompt.contains("Do not ask to confirm a plan"), "{prompt}");
    }

    #[test]
    fn names_the_platform() {
        let prompt = build(Path::new("/tmp"), None, None, false, false);
        let named = ["Windows", "macOS", "Linux"]
            .iter()
            .any(|p| prompt.contains(p));
        assert!(named, "the prompt must tell the model which OS it is on");
    }

    #[test]
    fn skill_authoring_guidance_follows_the_setting() {
        assert!(build(Path::new("/tmp"), None, None, true, false).contains("propose_skill"));
        assert!(!build(Path::new("/tmp"), None, None, false, false).contains("propose_skill"));
    }

    #[test]
    fn the_skill_catalog_is_included_when_present() {
        let prompt = build(
            Path::new("/tmp"),
            Some("# Skills\n\n- alpha: when alpha\n".into()),
            None,
            false,
            false,
        );
        assert!(prompt.contains("- alpha: when alpha"));
    }

    #[test]
    fn standing_instructions_land_after_the_rules_they_can_contradict() {
        // Order is the whole design here. A brief saying "ask before touching
        // the database" argues with "keep going until the task is done", and a
        // small model settles that by recency — so the brief has to come second
        // or it silently loses every time.
        let prompt = build(
            Path::new("/tmp"),
            Some("# Skills\n\n- alpha: when alpha\n".into()),
            Some("# Instructions\n\nAsk before touching the database.\n".into()),
            false,
            false,
        );
        let instructions = prompt.find("Ask before touching").expect("the brief");
        let base = prompt.find("Keep going until").expect("the base rule");
        let skills = prompt.find("- alpha: when alpha").expect("the catalog");
        assert!(base < instructions, "the brief must follow the base rules");
        assert!(
            instructions < skills,
            "reference material belongs after the rules"
        );
    }

    #[test]
    fn no_instructions_file_adds_no_section() {
        let prompt = build(Path::new("/tmp"), None, None, false, false);
        assert!(!prompt.contains("# Instructions"));
    }

    #[test]
    fn agent_authoring_guidance_follows_its_own_setting() {
        // Two settings, two sections. Turning skills off must not silently take
        // the agent guidance with it, or the model is offered `propose_agent`
        // with nothing telling it when to reach for one.
        assert!(build(Path::new("/tmp"), None, None, false, true).contains("propose_agent"));
        assert!(!build(Path::new("/tmp"), None, None, true, false).contains("propose_agent"));
    }

    #[test]
    fn the_two_authoring_sections_say_how_they_differ() {
        // The distinction a model gets wrong. An agent that should have been a
        // skill costs a roster line on every request for work it could have
        // done inline.
        let prompt = build(Path::new("/tmp"), None, None, true, true);
        assert!(prompt.contains("Propose a skill instead"), "{prompt}");
    }
}
