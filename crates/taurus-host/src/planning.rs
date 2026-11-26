//! Planning: a conversation that reads and writes a plan, and changes nothing
//! until the person approves it.
//!
//! # Enforced by removal
//!
//! While a conversation is planning, every tool that could change anything is
//! taken out of the turn's registry, rather than left in and refused at the
//! permission gate. Two reasons, and the second is the one that matters for
//! the models this harness is built around. It is the registry's own rule —
//! see [`ToolRegistry::remove`] — that "off" means not there, so nothing can
//! reach it by another route. And a small model offered `write_file` and then
//! refused it will spend its turn retrying the write; one never offered it
//! writes the plan. The price is the tool schemas changing once, on approval,
//! which costs a backend's cached prefix once per task.
//!
//! # What counts as changing something
//!
//! A tool's [`Effect`] says it, and the line is drawn at [`Effect::Read`] and
//! [`Effect::Network`]: looking at the workspace, and looking something up.
//! Tools marked `Read` that write into Taurus's own state — a memory note, a
//! proposed skill — stay. They change nothing the person is protecting, and
//! each is shown as it happens.
//!
//! The one tool whose effect does not say what it can do is the spawn tool.
//! It is `Read` because each child's calls are gated on their own, which is
//! true and beside the point here: a `coder` it starts would write. So a
//! planning turn's spawn tool is built with only the agents that read
//! ([`readers`]), and its children draw their tools from a registry that has
//! had the same removal applied ([`read_only`]). Either alone would do. Both
//! are here because the first depends on agent files declaring their tools
//! honestly, and the second does not.
//!
//! Commands are not offered at all, read-only ones included. `explorer` does
//! without them too, and no line of shell can be classified as harmless with
//! enough confidence to let it through a gate whose whole promise is that
//! nothing changed.

use taurus_agents::AgentDefinition;
use taurus_tools::{Effect, ToolRegistry};

/// The message that approves a plan, sent as the person's own.
///
/// One wording for every frontend, so a transcript reads the same whichever
/// one approved it, and so the model meets the same words each time. It is
/// sent as an ordinary message rather than a signal because that is what it
/// is: the person saying go, recorded where everything else they said is.
pub const APPROVED: &str = "The plan is approved. Carry it out.";

/// Whether a tool with this effect may be offered while planning.
pub fn allowed(effect: Effect) -> bool {
    matches!(effect, Effect::Read | Effect::Network)
}

/// Removes every tool that could change something, answering what it removed.
pub fn read_only(registry: &mut ToolRegistry) -> Vec<String> {
    let doomed: Vec<String> = registry
        .names()
        .filter(|name| {
            registry
                .get(name)
                .is_some_and(|tool| !allowed(tool.effect()))
        })
        .map(str::to_string)
        .collect();
    for name in &doomed {
        registry.remove(name);
    }
    doomed
}

/// The agents a planning turn may delegate to: those that list their tools,
/// and list none that changes anything.
///
/// An agent with no `tools:` key inherits everything its parent has, writers
/// included, so it is left out. A listed tool `registry` does not hold is not
/// held against the agent — `search_code` is absent wherever the index is off,
/// and that should not cost a planning turn its `explorer`. The child's
/// registry has been through [`read_only`] anyway, which is what makes the
/// leniency safe.
pub fn readers(agents: &[AgentDefinition], registry: &ToolRegistry) -> Vec<AgentDefinition> {
    agents
        .iter()
        .filter(|agent| {
            agent.frontmatter.tools.as_ref().is_some_and(|tools| {
                tools
                    .iter()
                    .all(|name| registry.get(name).is_none_or(|tool| allowed(tool.effect())))
            })
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_commands_go_reads_stay() {
        let mut registry = ToolRegistry::with_builtins();
        let removed = read_only(&mut registry);
        for name in ["write_file", "edit_file", "run_command", "stop_command"] {
            assert!(removed.iter().any(|r| r == name), "{name} was kept");
            assert!(registry.get(name).is_none());
        }
        for name in ["read_file", "list_dir", "glob", "grep", "check_command"] {
            assert!(registry.get(name).is_some(), "{name} was removed");
        }
    }

    #[test]
    fn only_agents_that_read_are_offered() {
        let registry = ToolRegistry::with_builtins();
        let agents = taurus_agents::builtin::definitions();
        let names: Vec<String> = readers(&agents, &registry)
            .iter()
            .map(|a| a.name().to_string())
            .collect();
        assert_eq!(names, [taurus_agents::builtin::EXPLORER]);
    }
}
