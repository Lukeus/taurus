//! The skills, agents and tools a turn can reach, and the prompt that describes them.

use super::*;

impl Host {
    pub fn catalog(&self) -> &SharedCatalog {
        &self.catalog
    }

    /// The live roster, for a caller re-checking a proposal before it is saved.
    /// [`Host::agents`] returns the summaries a drawer renders; this is the
    /// catalog itself.
    pub fn agent_catalog(&self) -> &SharedAgentCatalog {
        &self.agents
    }

    /// The live registry, for the same reason: a proposal naming a tool has to
    /// be checked against what this session actually has.
    pub fn registry(&self) -> &Arc<RwLock<ToolRegistry>> {
        &self.registry
    }

    pub async fn skills(&self) -> Vec<SkillSummary> {
        self.catalog.read().await.summaries()
    }

    /// The sub-agent roster: the built-ins, plus whatever the last reload found
    /// on disk, with anything that shadowed something else saying so.
    pub async fn agents(&self) -> Vec<AgentSummary> {
        self.agents.read().await.summaries()
    }

    /// Characters of every request the roster costs. Shown next to the roster,
    /// because a cost nobody can see is one nobody chooses.
    pub async fn roster_cost(&self) -> usize {
        self.agents.read().await.roster_cost()
    }

    pub async fn skill_count(&self) -> usize {
        self.catalog.read().await.len()
    }

    /// Resolves a leading `/name` into the skill or sub-agent it refers to.
    ///
    /// `None` for anything that is not a command, which is almost every
    /// message. Callers send the user's own text in that case — expansion is
    /// something that happens on the way to the model, and never changes what
    /// the transcript shows the user having typed.
    pub async fn expand_command(
        &self,
        text: &str,
    ) -> Option<Result<command::Invocation, command::CommandError>> {
        // This is the first thing a turn does, and the roster it resolves
        // against has to include the agent written since the last one — see
        // `refresh_for_turn`. Deliberately not in `commands()` beside it: that
        // one answers a keystroke, and taking config write locks on the
        // completion path is how a reload comes to deadlock against typing.
        self.refresh_for_turn().await;
        self.rosters(|rosters| rosters.expand(text)).await
    }

    /// Skills and sub-agents a person can run as `/name`, for completion as
    /// they type.
    ///
    /// Lists what the last scan found rather than rescanning: this is a
    /// keystroke path. An agent written a moment ago is missing from the menu
    /// until something rescans — the next turn does, and so does opening the
    /// Agents drawer — but typing its name in full works immediately, because
    /// [`Self::expand_command`] refreshes before it resolves.
    pub async fn commands(&self) -> Vec<command::CommandSummary> {
        self.rosters(|rosters| rosters.summaries()).await
    }

    /// Borrows both catalogs at once for the slash namespace.
    ///
    /// A closure rather than a returned `Rosters` because the two guards have
    /// to outlive it, and holding both across an `.await` in a caller is how a
    /// reload deadlocks against a keystroke.
    ///
    /// The rule this file keeps everywhere, stated once here: no read guard is
    /// held across an `.await`. tokio's `RwLock` is write-preferring, so a guard
    /// held across one while a writer queues blocks every new reader behind
    /// that writer — and a second lock taken in the other order is a deadlock.
    /// Bind the snapshot to a local, then await the next thing.
    pub(super) async fn rosters<T>(&self, f: impl FnOnce(command::Rosters<'_>) -> T) -> T {
        // Read here rather than passed in: whether a turn can delegate is a
        // setting, and the composer asking what it may offer should get the
        // same answer the send path will act on. Read first, so the two guards
        // below are never held across an await.
        let can_delegate = !self
            .settings
            .read()
            .await
            .disabled_tools
            .iter()
            .any(|tool| tool == taurus_core::SPAWN_TOOL);
        let skills = self.catalog.read().await;
        let agents = self.agents.read().await;
        f(command::Rosters {
            skills: &skills,
            agents: &agents,
            can_delegate,
        })
    }

    /// Every directory the last scan read, in precedence order.
    ///
    /// Resolved rather than described, so an empty library can be explained
    /// with the paths actually consulted — including the shared locations,
    /// which the user did not configure and may not know are read.
    pub async fn skill_sources(&self) -> Vec<taurus_skills::SkillSource> {
        config::skill_sources(Some(&self.workspace.read().await.clone()))
    }

    /// The standing brief in force, in the order it reaches the prompt.
    ///
    /// Exposed for the same reason the skill roster is: a file being read is
    /// invisible otherwise, and "why is it doing that" has no answer if the
    /// user cannot see which briefs are loaded.
    pub async fn instructions(&self) -> Vec<Instructions> {
        self.instructions.read().await.clone()
    }

    pub async fn tool_names(&self) -> Vec<String> {
        self.registry
            .read()
            .await
            .names()
            .map(str::to_string)
            .collect()
    }

    /// What the model is told it can call, as it goes over the wire.
    ///
    /// The same list [`Host::build_agent`] would hand a turn, minus the spawn
    /// tool that is added per turn. Exposed so the cost of advertising it can be
    /// reported: this is the part of every request that is fixed overhead, paid
    /// again on each iteration whether or not a tool is called.
    pub async fn tool_definitions(&self) -> Vec<taurus_provider::ToolDef> {
        self.registry.read().await.definitions()
    }

    /// The system prompt a turn in this workspace would carry.
    pub async fn system_prompt(&self) -> String {
        let workspace = self.workspace.read().await.clone();
        // No session to exclude: this reports what a turn would carry, and the
        // turn that will carry it has not started.
        let memory_section = memory::section(&memory::load(&workspace), "");
        // Each bound before the next await rather than read inline as an
        // argument, where every guard lives until the call — see `Self::rosters`.
        let skills = self.catalog.read().await.prompt_section();
        let instructions = instructions::section(&self.instructions.read().await);
        let (synthesis, agent_synthesis) = {
            let settings = self.settings.read().await;
            (
                settings.skill_synthesis_enabled,
                settings.agent_synthesis_enabled,
            )
        };
        prompt::build(
            &workspace,
            skills,
            instructions,
            memory_section,
            synthesis,
            agent_synthesis,
        )
    }
}
