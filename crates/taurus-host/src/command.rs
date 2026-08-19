//! The `/name` namespace, over both catalogs.
//!
//! A slash command names a skill or a sub-agent, and neither catalog can tell
//! which on its own — so resolution lives above both rather than inside either.
//! What a name expands to differs; how it is typed does not. A skill's
//! procedure replaces the user's line, and an agent's name becomes an
//! instruction to delegate, because a turn is run by the main agent and handing
//! work to a child is something it does with a tool.
//!
//! Skills win a name they share with an agent. The rule is not a judgement
//! about which is more useful: `/review` ran a skill before agents were on the
//! slash key, and a command that quietly starts doing something else is worse
//! than a name that is awkward to reach.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use taurus_agents::{AgentCatalog, AgentDefinition};
use taurus_skills::SkillCatalog;

/// Which catalog a `/name` came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum CommandKind {
    Skill,
    Agent,
}

/// One row of the composer's completion menu.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommandSummary {
    pub name: String,
    pub kind: CommandKind,
    /// The one line the menu shows: a skill's trigger, an agent's description.
    /// Both fields are written to answer the same question, which is why one
    /// field can carry either.
    pub when_to_use: String,
}

/// A resolved `/name args`.
#[derive(Clone, Debug)]
pub struct Invocation {
    /// What matched, by its own name rather than what was typed.
    pub name: String,
    pub kind: CommandKind,
    /// What the model receives in place of the user's line.
    pub prompt: String,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum CommandError {
    #[error("There is no skill or agent named '{name}'.{}", suggest(.suggestions))]
    Unknown {
        name: String,
        suggestions: Vec<String>,
    },

    #[error("The skill '{name}' is not meant to be run directly; it says so in its frontmatter.")]
    NotUserInvocable { name: String },

    #[error(
        "'/{name}' hands the task to the '{name}' sub-agent, which needs the {tool} tool — and \
         this session has it disabled. Take {tool} out of disabled_tools to run agents as \
         commands."
    )]
    DelegationDisabled { name: String, tool: &'static str },
}

/// The two rosters a `/name` is resolved against.
///
/// Borrowed together rather than passed one at a time because the answer
/// depends on all three fields at once: which name wins, and whether the agent
/// half of the namespace exists at all this session.
pub struct Rosters<'a> {
    pub skills: &'a SkillCatalog,
    pub agents: &'a AgentCatalog,
    /// Whether `spawn_subagent` is available. An agent is only runnable as a
    /// command when it is — the expansion is an instruction to call that tool,
    /// so a session that disabled it would otherwise get a prompt naming a tool
    /// the model does not have, and spend a turn discovering that.
    pub can_delegate: bool,
}

impl Rosters<'_> {
    /// Expands `/name args` into what the user asked for.
    ///
    /// Returns `None` when the text is not a command at all, which is the
    /// common case and must stay cheap and unsurprising: a message that merely
    /// begins with a slash — a path, a fraction, a closing tag — is left
    /// exactly as typed.
    pub fn expand(&self, text: &str) -> Option<Result<Invocation, CommandError>> {
        let (name, args) = split_command(text)?;
        Some(self.resolve(name, args))
    }

    fn resolve(&self, name: &str, args: &str) -> Result<Invocation, CommandError> {
        let skill = self.skills.get(name);

        if let Some(skill) = skill.filter(|s| s.frontmatter.user_invocable) {
            return Ok(Invocation {
                name: skill.name().to_string(),
                kind: CommandKind::Skill,
                prompt: skill.render(args),
            });
        }

        // Checked before the skill's own refusal below, so a model-only skill
        // sharing a name with an agent leaves the name usable rather than
        // reserving it for something it will not run.
        if let Some(agent) = self.agents.get(name) {
            if !self.can_delegate {
                return Err(CommandError::DelegationDisabled {
                    name: agent.name().to_string(),
                    tool: taurus_core::SPAWN_TOOL,
                });
            }
            return Ok(Invocation {
                name: agent.name().to_string(),
                kind: CommandKind::Agent,
                prompt: delegate(agent, args),
            });
        }

        if skill.is_some() {
            return Err(CommandError::NotUserInvocable {
                name: name.to_string(),
            });
        }

        // Near misses rather than the whole library: the reason a command fails
        // is almost always a half-remembered name, and a list of everything
        // installed is a worse answer than the two it might have been.
        let mut suggestions: Vec<String> = self
            .summaries()
            .into_iter()
            .map(|c| c.name)
            .filter(|known| known.contains(name) || name.contains(known.as_str()))
            .collect();
        suggestions.sort();
        Err(CommandError::Unknown {
            name: name.to_string(),
            suggestions,
        })
    }

    /// Everything runnable as `/name`, for completion as the user types.
    ///
    /// A name held by both is listed once, as the skill, matching which one
    /// [`Rosters::expand`] would run — a menu row that runs something other
    /// than what it says is worse than a missing row.
    pub fn summaries(&self) -> Vec<CommandSummary> {
        let mut out: Vec<CommandSummary> = self
            .skills
            .commands()
            .map(|skill| CommandSummary {
                name: skill.name().to_string(),
                kind: CommandKind::Skill,
                when_to_use: skill.trigger(),
            })
            .collect();

        if self.can_delegate {
            let taken: Vec<&str> = out.iter().map(|c| c.name.as_str()).collect();
            let agents: Vec<CommandSummary> = self
                .agents
                .iter()
                .filter(|agent| !taken.contains(&agent.name()))
                .map(|agent| CommandSummary {
                    name: agent.name().to_string(),
                    kind: CommandKind::Agent,
                    when_to_use: agent.frontmatter.description.clone(),
                })
                .collect();
            out.extend(agents);
        }
        out
    }
}

/// The prompt an `/agent-name` expands to.
///
/// Short on purpose. This lands as the user's message on a turn whose model may
/// have an 8k context, and every line spent explaining delegation is a line not
/// spent on the task. The three facts the model cannot infer are which agent,
/// that the child sees none of this conversation, and that its answer is what
/// to report back.
fn delegate(agent: &AgentDefinition, args: &str) -> String {
    let name = agent.name();
    let tool = taurus_core::SPAWN_TOOL;
    let mut out = format!(
        "Hand this to the `{name}` sub-agent: call `{tool}` with agent_type \"{name}\", and \
         report what it returns. It shares none of this conversation and cannot ask you \
         questions, so write the whole task into its prompt.\n\n"
    );

    let args = args.trim();
    if args.is_empty() {
        // A bare `/explorer` is a follow-up — "now do that with the explorer" —
        // and the task it means is the one already on the table. Saying so
        // beats sending an empty instruction the model has to guess around.
        out.push_str("The task is what this conversation has established so far.\n");
    } else {
        out.push_str(&format!("The task:\n\n{args}\n"));
    }
    out
}

fn suggest(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => format!(" Did you mean /{one}?"),
        many => format!(
            " Did you mean one of {}?",
            many.iter()
                .map(|n| format!("/{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Splits `/name rest` into its two halves, or `None` if this is not a command.
///
/// Two rules keep ordinary text out, and both were written against real
/// messages rather than in the abstract. The name must be followed by
/// whitespace or nothing, so `/usr/bin/env is portable` stays prose. And it
/// must begin with a letter, so `/2 of the tests fail` does too — the skill
/// specification allows a name to start with a digit, but a message starting
/// with a slash and a number is a fraction or a date far more often than it is
/// a command nobody has written yet.
///
/// Getting this wrong is asymmetric. Failing to recognize a command shows the
/// user their own text; recognizing one that was not there refuses to send what
/// they wrote.
fn split_command(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('/')?;
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (name, args) = rest.split_at(end);

    let plausible = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    plausible.then_some((name, args))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use taurus_agents::{AgentSource, AgentTier};
    use taurus_skills::{SkillOrigin, SkillSource, SkillTier};

    use super::*;

    fn write_skill(root: &Path, name: &str, body: &str, extra: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: does {name}\n{extra}---\n\n{body}\n"),
        )
        .unwrap();
    }

    fn write_agent(root: &Path, name: &str) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(
            root.join(format!("{name}.md")),
            format!("---\nname: {name}\ndescription: reviews {name} things\n---\n\nBe {name}.\n"),
        )
        .unwrap();
    }

    fn skills(dir: &Path) -> SkillCatalog {
        SkillCatalog::discover(&[SkillSource {
            tier: SkillTier::User,
            origin: SkillOrigin::Taurus,
            dir: dir.to_path_buf(),
        }])
        .0
    }

    fn agents(dir: &Path) -> AgentCatalog {
        AgentCatalog::discover(&[AgentSource {
            borrowed: false,
            tier: AgentTier::User,
            dir: dir.to_path_buf(),
        }])
        .0
    }

    /// A workspace with both libraries in it, borrowed as one roster.
    struct Fixture {
        skills: SkillCatalog,
        agents: AgentCatalog,
        _dirs: (TempDir, TempDir),
    }

    impl Fixture {
        fn new(build: impl FnOnce(&Path, &Path)) -> Self {
            let (skill_dir, agent_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
            build(skill_dir.path(), agent_dir.path());
            Self {
                skills: skills(skill_dir.path()),
                agents: agents(agent_dir.path()),
                _dirs: (skill_dir, agent_dir),
            }
        }

        fn rosters(&self) -> Rosters<'_> {
            Rosters {
                skills: &self.skills,
                agents: &self.agents,
                can_delegate: true,
            }
        }
    }

    #[test]
    fn ordinary_text_that_begins_with_a_slash_is_not_a_command() {
        let fixture = Fixture::new(|skills, _| write_skill(skills, "usr", "Do usr things.", ""));

        for text in [
            "/usr/bin/env is the portable way",
            "/2 of the tests fail",
            "look in /etc",
            "/",
            "//comment",
            "/-leading-dash",
            "/Not_A_Skill do the thing",
        ] {
            assert!(
                fixture.rosters().expand(text).is_none(),
                "'{text}' must be sent as written"
            );
        }
    }

    #[test]
    fn a_command_expands_to_the_skill_with_its_arguments() {
        let fixture = Fixture::new(|skills, _| {
            write_skill(
                skills,
                "speckit-specify",
                "Build a spec from:\n\n$ARGUMENTS",
                "",
            )
        });

        let invocation = fixture
            .rosters()
            .expand("/speckit-specify add a dark mode toggle")
            .expect("this is a command")
            .expect("and the skill exists");

        assert_eq!(invocation.name, "speckit-specify");
        assert_eq!(invocation.kind, CommandKind::Skill);
        assert!(invocation.prompt.contains("Build a spec from:"));
        assert!(
            invocation.prompt.contains("add a dark mode toggle"),
            "the placeholder must be filled: {}",
            invocation.prompt
        );
        assert!(
            !invocation.prompt.contains("$ARGUMENTS"),
            "no placeholder may survive into the prompt"
        );
    }

    #[test]
    fn a_command_with_no_placeholder_still_carries_what_was_typed() {
        let fixture =
            Fixture::new(|skills, _| write_skill(skills, "review", "Review the code.", ""));

        let invocation = fixture
            .rosters()
            .expand("/review the auth module")
            .unwrap()
            .unwrap();
        assert!(
            invocation.prompt.contains("the auth module"),
            "dropping the request because the author wrote no placeholder loses the ask"
        );
    }

    #[test]
    fn an_agent_name_expands_to_an_instruction_to_delegate() {
        let fixture = Fixture::new(|_, agents| write_agent(agents, "reviewer"));

        let invocation = fixture
            .rosters()
            .expand("/reviewer check the auth module")
            .expect("this is a command")
            .expect("and the agent exists");

        assert_eq!(invocation.name, "reviewer");
        assert_eq!(invocation.kind, CommandKind::Agent);
        assert!(invocation.prompt.contains("spawn_subagent"));
        assert!(
            invocation.prompt.contains("\"reviewer\""),
            "the agent_type has to be quoted the way the tool takes it: {}",
            invocation.prompt
        );
        assert!(invocation.prompt.contains("check the auth module"));
    }

    #[test]
    fn a_builtin_agent_is_reachable_without_a_file() {
        // The roster a fresh machine has is the built-ins, and they are the
        // agents most people will type first.
        let fixture = Fixture::new(|_, _| {});
        let invocation = fixture
            .rosters()
            .expand("/explorer find every caller of build_agent")
            .unwrap()
            .unwrap();
        assert_eq!(invocation.name, "explorer");
        assert_eq!(invocation.kind, CommandKind::Agent);
    }

    #[test]
    fn an_agent_with_no_arguments_is_pointed_at_the_conversation() {
        let fixture = Fixture::new(|_, _| {});
        let invocation = fixture.rosters().expand("/explorer").unwrap().unwrap();
        assert!(
            invocation.prompt.contains("conversation has established"),
            "an empty task has to say what it means: {}",
            invocation.prompt
        );
    }

    #[test]
    fn a_skill_wins_a_name_it_shares_with_an_agent() {
        let fixture = Fixture::new(|skills, agents| {
            write_skill(skills, "explorer", "Explore, as a procedure.", "");
            write_agent(agents, "explorer");
        });

        let invocation = fixture.rosters().expand("/explorer go").unwrap().unwrap();
        assert_eq!(
            invocation.kind,
            CommandKind::Skill,
            "a name that ran a skill yesterday must still run it today"
        );

        let names: Vec<String> = fixture
            .rosters()
            .summaries()
            .into_iter()
            .filter(|c| c.name == "explorer")
            .map(|c| format!("{:?}", c.kind))
            .collect();
        assert_eq!(names, ["Skill"], "and the menu must not offer it twice");
    }

    #[test]
    fn an_agent_can_have_a_name_a_model_only_skill_holds() {
        // The skill will not run from here anyway, so reserving the name for it
        // would leave the agent unreachable for no gain.
        let fixture = Fixture::new(|skills, agents| {
            write_skill(skills, "worker", "Work.", "user-invocable: false\n");
            write_agent(agents, "worker");
        });

        let invocation = fixture
            .rosters()
            .expand("/worker ship it")
            .unwrap()
            .unwrap();
        assert_eq!(invocation.kind, CommandKind::Agent);
    }

    #[test]
    fn a_model_only_skill_with_no_agent_behind_it_says_why_it_will_not_run() {
        let fixture = Fixture::new(|skills, _| {
            write_skill(skills, "model-only", "Internal.", "user-invocable: false\n")
        });

        assert!(matches!(
            fixture.rosters().expand("/model-only").unwrap(),
            Err(CommandError::NotUserInvocable { .. })
        ));
    }

    #[test]
    fn an_unknown_command_suggests_names_from_both_rosters() {
        let fixture = Fixture::new(|skills, agents| {
            write_skill(skills, "speckit-specify", "Spec.", "");
            write_agent(agents, "code-reviewer");
        });

        let rosters = fixture.rosters();
        let message = rosters.expand("/specify").unwrap().unwrap_err().to_string();
        assert!(
            message.contains("no skill or agent named 'specify'"),
            "{message}"
        );
        assert!(message.contains("/speckit-specify"), "{message}");

        let message = rosters
            .expand("/reviewer")
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("/code-reviewer"),
            "an agent is a suggestion too: {message}"
        );
    }

    #[test]
    fn agents_leave_the_namespace_when_delegation_is_disabled() {
        let fixture = Fixture::new(|skills, _| write_skill(skills, "review", "Review.", ""));
        let rosters = Rosters {
            can_delegate: false,
            ..fixture.rosters()
        };

        assert!(
            rosters
                .summaries()
                .iter()
                .all(|c| c.kind == CommandKind::Skill),
            "offering a completion the harness would then refuse is a dead end typed in full"
        );

        // But typing one anyway is answered with the reason rather than
        // "no such command", which would send the user looking for a typo.
        let message = rosters
            .expand("/explorer look")
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(message.contains("disabled_tools"), "{message}");
    }

    #[test]
    fn the_menu_offers_both_kinds() {
        let fixture = Fixture::new(|skills, agents| {
            write_skill(skills, "review", "Review.", "");
            write_agent(agents, "auditor");
        });

        let summaries = fixture.rosters().summaries();
        let review = summaries.iter().find(|c| c.name == "review").unwrap();
        assert_eq!(review.kind, CommandKind::Skill);
        let auditor = summaries.iter().find(|c| c.name == "auditor").unwrap();
        assert_eq!(auditor.kind, CommandKind::Agent);
        assert_eq!(auditor.when_to_use, "reviews auditor things");
    }

    #[test]
    fn a_user_only_skill_is_offered_and_a_model_only_one_is_not() {
        let fixture = Fixture::new(|skills, _| {
            write_skill(
                skills,
                "user-only",
                "Ask me.",
                "disable-model-invocation: true\n",
            );
            write_skill(skills, "model-only", "Internal.", "user-invocable: false\n");
        });

        let offered: Vec<String> = fixture
            .rosters()
            .summaries()
            .into_iter()
            .filter(|c| c.kind == CommandKind::Skill)
            .map(|c| c.name)
            .collect();
        assert_eq!(offered, ["user-only"]);
    }
}
