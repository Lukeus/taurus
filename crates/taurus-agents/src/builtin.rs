//! The agents that ship with the harness.
//!
//! Built in Rust and never read from disk, so an unreadable or absent `agents`
//! directory cannot take them away — the same reasoning that makes the
//! delegation depth cap structural rather than a counter. A user file of the
//! same name shadows one of these; nothing can delete it.
//!
//! # Why three, and how they divide
//!
//! The roster is fixed overhead in the spawn tool's description, so a fourth
//! line has to earn itself against the three already there — and the way it
//! earns itself is by being *pickable*. Two agents whose descriptions both
//! amount to "makes changes" are worse than one, because the parent chooses
//! from those lines alone and now has to guess. So the split here is a question
//! the parent can answer about its own task before it delegates:
//!
//! - `explorer` — can it be answered by reading? Nothing it does can be undone.
//! - `worker` — can you dictate the edit? Then dictate it; nothing is left open.
//! - `coder` — does someone have to look at the code and decide? That is this
//!   one, and it is the only one that goes and checks its own work.
//!
//! `coder` and `worker` overlap on purpose at the boundary, because the real
//! tasks do. What keeps them distinct in the description is *who decides*: a
//! `worker` task that turns out to need a judgement call has been mis-sent, and
//! a `coder` task that was fully specified merely spent a build on proving it.

use crate::agent::{AgentDefinition, AgentFrontmatter, AgentTier};

pub const EXPLORER: &str = "explorer";
pub const WORKER: &str = "worker";
pub const CODER: &str = "coder";

/// What `coder` is offered.
///
/// Named rather than inherited, unlike `worker`. Two reasons, and the second is
/// the one that matters: an explicit list is what makes the roster line true —
/// "reads what it needs, writes it, then builds or tests it" is a claim about
/// scope, and an agent that inherited a web client and an MCP server would be
/// making a different one. And a narrower set is a smaller thing to reason
/// about when a delegation goes wrong.
///
/// Every name here is a built-in tool that always exists, so the scope on the
/// roster and the scope in the registry agree on every machine. `search_code`
/// is deliberately absent for that reason: it appears only once a workspace has
/// been indexed, and an agent whose advertised reach depends on that is one
/// whose behaviour changes without anything in this file changing.
const CODER_TOOLS: [&str; 8] = [
    "read_file",
    "write_file",
    "edit_file",
    "list_dir",
    "glob",
    "grep",
    "run_command",
    "load_skill",
];

pub fn definitions() -> Vec<AgentDefinition> {
    vec![
        AgentDefinition {
            frontmatter: AgentFrontmatter {
                name: EXPLORER.into(),
                description: "Searches and reads the codebase to answer a question. Cannot modify \
                              anything. Use this when finding the answer would mean reading many \
                              files."
                    .into(),
                tools: Some(
                    ["read_file", "list_dir", "glob", "grep", "load_skill"]
                        .map(String::from)
                        .to_vec(),
                ),
                max_iterations: 20,
                model: None,
                provider: None,
            },
            system_prompt: "You are a research sub-agent. Search and read to answer the question \
                            you were given, then reply with the answer and the paths that support \
                            it. You cannot modify files. Be specific and brief; the agent that \
                            called you sees only your reply, not your tool calls."
                .into(),
            tier: AgentTier::Builtin,
            path: None,
            borrowed: false,
            shadows: None,
            degraded: None,
        },
        AgentDefinition {
            frontmatter: AgentFrontmatter {
                name: WORKER.into(),
                // Says "exactly-specified" where it used to say "well-specified",
                // which is the whole of the change: the old wording described
                // `coder` equally well, and a parent choosing between two lines
                // that both fit picks by coin toss.
                description: "Carries out an exactly-specified, self-contained change. Give it \
                              complete instructions — it cannot ask questions, and it will not \
                              decide anything you leave open."
                    .into(),
                // Absent, not empty: `worker` has always inherited the parent's
                // tools, and that is what `None` means.
                tools: None,
                max_iterations: 25,
                model: None,
                provider: None,
            },
            system_prompt: "You are a sub-agent carrying out one specific task. You cannot ask \
                            questions, so work from the instructions you were given. If they turn \
                            out not to cover something you have to decide, stop and say what is \
                            missing rather than guessing. When done, reply with what you changed. \
                            Be brief; the agent that called you sees only your reply."
                .into(),
            tier: AgentTier::Builtin,
            path: None,
            borrowed: false,
            shadows: None,
            degraded: None,
        },
        AgentDefinition {
            frontmatter: AgentFrontmatter {
                name: CODER.into(),
                description: "Implements a change in code: reads what it needs, writes it, then \
                              builds or tests it. Use when the work needs judgement about the \
                              code rather than an exact edit you could dictate."
                    .into(),
                tools: Some(CODER_TOOLS.map(String::from).to_vec()),
                // More than `worker`'s, because this one spends iterations the
                // others do not: reading around the change before it starts,
                // and a build or a test run afterwards that may fail and need
                // fixing. Still well under the 50 a file may ask for.
                max_iterations: 30,
                model: None,
                provider: None,
            },
            // The instruction to check its own work is here rather than left to
            // the agent loop's verify nudge, which a sub-agent does get. The
            // nudge fires once, at the end, when the model has already decided
            // it is finished; a coding agent that knows from the first iteration
            // that it will have to build spends its budget differently.
            system_prompt: "You are a coding sub-agent. Read enough of the code to make the \
                            change fit before you write any of it: the surrounding file is the \
                            specification for naming, error handling, and how much to comment. \
                            Then make the change and check it — build it, run the tests, or run \
                            the thing you changed. You cannot ask questions, so where the task \
                            leaves something open, take the option most consistent with the code \
                            already there and say which you took. Reply with what you changed, \
                            what you ran, and what it said; if you could not check it, say that \
                            instead of implying you did. Be brief; the agent that called you sees \
                            only your reply, not your tool calls."
                .into(),
            tier: AgentTier::Builtin,
            path: None,
            borrowed: false,
            shadows: None,
            degraded: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::validate;

    #[test]
    fn the_builtins_satisfy_the_rules_a_user_file_must() {
        // A built-in that a user could not have written on disk would mean two
        // sets of rules, and the second one undocumented.
        for agent in definitions() {
            validate(&agent.frontmatter, &agent.system_prompt, agent.name())
                .unwrap_or_else(|e| panic!("built-in '{}' is invalid: {e}", agent.name()));
        }
    }

    #[test]
    fn worker_inherits_its_tools_and_explorer_does_not() {
        let agents = definitions();
        let worker = agents.iter().find(|a| a.name() == WORKER).unwrap();
        let explorer = agents.iter().find(|a| a.name() == EXPLORER).unwrap();
        assert!(worker.frontmatter.tools.is_none());
        assert!(explorer
            .frontmatter
            .tools
            .as_ref()
            .is_some_and(|t| t.contains(&"read_file".to_string())));
    }

    fn find(name: &str) -> AgentDefinition {
        definitions()
            .into_iter()
            .find(|a| a.name() == name)
            .unwrap()
    }

    #[test]
    fn coder_can_read_write_and_run_and_nothing_else() {
        // The scope its own description claims. `explorer` cannot write and
        // `coder` cannot reach the network — both are the roster line being
        // true, since `tools:` is enforced rather than advisory.
        let tools = find(CODER).frontmatter.tools.expect("a named scope");
        for named in ["read_file", "edit_file", "write_file", "run_command"] {
            assert!(tools.contains(&named.to_string()), "coder needs {named}");
        }
        for withheld in ["fetch_url", "web_search", "propose_agent", "propose_skill"] {
            assert!(
                !tools.contains(&withheld.to_string()),
                "coder has {withheld}"
            );
        }
    }

    #[test]
    fn coder_is_told_to_check_its_own_work() {
        // The one thing that makes it more than a renamed `worker`. A coding
        // agent that reports a change it never built is reporting a guess.
        let prompt = find(CODER).system_prompt;
        assert!(prompt.contains("check it"), "{prompt}");
        assert!(prompt.contains("could not check it"), "{prompt}");
    }

    #[test]
    fn coder_and_worker_do_not_describe_the_same_job() {
        // What the parent picks from, and the only text it sees. If both lines
        // fit the task in front of it, the roster has three entries and two
        // answers. The split is who decides: `worker` is handed the decision,
        // `coder` makes it.
        let worker = find(WORKER).frontmatter.description;
        let coder = find(CODER).frontmatter.description;
        assert!(worker.contains("exactly-specified"), "{worker}");
        assert!(worker.contains("will not decide"), "{worker}");
        assert!(coder.contains("judgement"), "{coder}");
    }

    #[test]
    fn the_roster_stays_cheap_enough_to_send_every_time() {
        // Every description is sent on every request. The limit is checked by
        // `validate` per agent; this is the total the third one grew it to,
        // pinned so a fourth is a decision rather than a drift.
        let total: usize = definitions()
            .iter()
            .map(|a| a.frontmatter.description.chars().count())
            .sum();
        assert!(total < 600, "the roster costs {total} characters a request");
    }
}
