//! The agents that ship with the harness.
//!
//! Built in Rust and never read from disk, so an unreadable or absent `agents`
//! directory cannot take them away — the same reasoning that makes the
//! delegation depth cap structural rather than a counter. A user file of the
//! same name shadows one of these; nothing can delete it.

use crate::agent::{AgentDefinition, AgentFrontmatter, AgentTier};

pub const EXPLORER: &str = "explorer";
pub const WORKER: &str = "worker";

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
            shadows: None,
            degraded: None,
        },
        AgentDefinition {
            frontmatter: AgentFrontmatter {
                name: WORKER.into(),
                description: "Carries out a well-specified, self-contained change. Give it \
                              complete instructions; it cannot ask you questions."
                    .into(),
                // Absent, not empty: `worker` has always inherited the parent's
                // tools, and that is what `None` means.
                tools: None,
                max_iterations: 25,
                model: None,
                provider: None,
            },
            system_prompt: "You are a sub-agent carrying out one specific task. You cannot ask \
                            questions, so work from the instructions you were given. When done, \
                            reply with what you changed. Be brief; the agent that called you sees \
                            only your reply."
                .into(),
            tier: AgentTier::Builtin,
            path: None,
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
}
