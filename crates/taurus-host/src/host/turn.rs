//! What a turn is assembled from: the agent, its tools, its checkpoints, and the work running beside it.

use super::*;

impl Host {
    /// Builds the agent for one turn.
    ///
    /// Re-reads config first — see [`Self::refresh_for_turn`] for why a turn
    /// boundary is where that belongs.
    ///
    /// The single place system prompt, tool set, sub-agent wiring, and the
    /// turn's checkpoint come together, so the CLI and the desktop app cannot
    /// disagree about how an agent is configured — or about whether a turn is
    /// rewindable.
    pub async fn build_agent(
        &self,
        provider: Arc<dyn Provider>,
        model: &str,
        cancel: CancellationToken,
        turn: TurnRef<'_>,
    ) -> Agent {
        // Before anything is read out of the host, so the snapshot below and
        // the prompt built further down both see the same, current config.
        self.refresh_for_turn().await;

        // Started here rather than when the workspace opened: nobody's machine
        // should embed a repository because they looked at it, and a turn is
        // the point where a search becomes likely. It has the length of the
        // model's first few tool calls to get ahead, and `search_code` takes
        // over whatever is left. See [`Self::warm_index`].
        self.warm_index().await;

        // What every agent in this turn runs with: the parent, and each child it
        // delegates to. Read per turn rather than captured once, so raising the
        // iteration ceiling in Settings applies to the next message instead of
        // the next launch.
        //
        // Content capture follows the same rule, and it matters more there:
        // turning it off has to take effect on the next message rather than the
        // next launch, or somebody who has just realized what they switched on
        // cannot switch it off.
        let base = {
            let settings = self.settings.read().await;
            AgentConfig {
                max_iterations: settings.max_iterations,
                capture: if settings.otlp_capture_content {
                    taurus_core::Capture::Content
                } else {
                    taurus_core::Capture::MetadataOnly
                },
                ..Default::default()
            }
        };

        // Bound to this session's provider and model, so it is added per turn
        // rather than living in the shared registry. Children get the shared
        // registry, which has no spawn tool — that is the depth cap.
        let mut registry = self.registry.read().await.clone();
        // The roster is snapshotted here, so a turn sees the set of agents it
        // started with even if a file is saved while it runs. Each read is
        // bound before the next await, so no guard is held across one — see
        // `Self::rosters`.
        let roster = Arc::new(self.agents.read().await.to_vec());
        let models = self.agent_models.read().await.clone();
        let logs = SubagentLogs::new(self.workspace().await, turn.session_id);
        registry.register(Arc::new(
            SpawnSubagent::new(
                provider.clone(),
                self.registry.clone(),
                model,
                MAX_CONCURRENT_SUBAGENTS,
            )
            .with_defaults(base.clone())
            .with_roster(roster, models)
            // Every child's conversation is kept, under this one's. The parent
            // transcript still records a delegation as one call and one answer
            // — that is what delegating is for — but the work behind that
            // answer is no longer thrown away with the tool result.
            .with_recorder(Arc::new(logs)),
        ));

        // Registered per turn, alongside the spawn tool and for the same
        // reason: these address the person watching this conversation, and a
        // sub-agent has no such person. It shares the registry above, which is
        // what keeps `ask_user` away from a worker that cannot ask anyone
        // anything, and keeps a delegate from drawing a chart into a transcript
        // it is not part of.
        registry.register(Arc::new(ShowTable));
        registry.register(Arc::new(ShowChart));
        registry.register(Arc::new(ShowSequence));
        registry.register(Arc::new(ShowFlow));
        registry.register(Arc::new(AskUser::new(self.asker.clone())));
        // Per turn with the rest of them, and the argument is the sharpest
        // here: the canvas is one pane in one window belonging to one
        // conversation. A delegate opening a file would put it on a screen
        // nobody had asked to change, in the middle of work they cannot see.
        registry.register(Arc::new(OpenFile));

        // The *tool* is per turn, like the three above: a delegate writing into
        // the parent's checklist would report progress against a task nobody
        // gave it. The *board* is per conversation, so an unfinished plan
        // survives the message that interrupted it — `start_turn` is what drops
        // a finished one, and is the whole of the staleness rule.
        let plan = self.plan_board(turn.session_id).await;
        plan.start_turn();
        registry.register(Arc::new(UpdatePlan::new(plan.clone())));

        // Per turn for the mechanical reason the rest of this block is: a note
        // names the conversation that wrote it, and the shared registry a child
        // inherits has no session id to name.
        //
        // So this is the parent's tool only, and that is the right place for it
        // rather than a limitation to work around. A delegate's conclusion
        // comes back as its answer; the parent is the one that can see it
        // beside everything else the turn learned and judge whether it outlives
        // the conversation. A worker writing directly into a workspace's memory
        // would file what it found without knowing whether it mattered.
        registry.register(Arc::new(memory::Remember::new(
            self.workspace().await,
            turn.session_id,
        )));

        // `reload` applies this to the shared registry, which is everything
        // registered *there* — so without a second pass here, the per-turn
        // tools were the one set `disabled_tools` could not reach, and the
        // guarantee that a disabled tool is not registered at all held for
        // every tool but the four the parent turn adds for itself. Silent, and
        // exactly the direction that matters: a name typed to take a tool away
        // that quietly leaves it on.
        //
        // Unmatched names are not reported from here. `reload` already reports
        // them once, against the full set including these, and a turn is not a
        // place to raise a configuration problem — it would arrive once per
        // message for as long as the typo lived.
        disable(
            &mut registry,
            &self.settings.read().await.disabled_tools.clone(),
        );

        let workspace = self.workspace.read().await.clone();
        let skill_section = self.catalog.read().await.prompt_section();
        let instructions_section = instructions::section(&self.instructions.read().await);
        // This conversation's own notes are left out — see `memory::section`.
        let memory_section = memory::section(&memory::load(&workspace), turn.session_id);
        let synthesis = self.settings.read().await.skill_synthesis_enabled;
        let agent_synthesis = self.settings.read().await.agent_synthesis_enabled;

        // Opened here rather than by the loop, because a sub-agent runs its own
        // loop and must record into the turn that spawned it, not one of its
        // own. Cloning the context is what carries it down.
        let recorder =
            self.checkpoints()
                .await
                .begin_turn(turn.session_id, &workspace, turn.prompt);

        Agent::new(
            provider,
            registry,
            {
                let context = self
                    .tool_context(cancel)
                    .await
                    .with_checkpoints(recorder)
                    .with_session(turn.session_id);
                match turn.unattended {
                    Some(switch) => context.with_unattended(switch),
                    None => context,
                }
            },
            AgentConfig {
                system_prompt: prompt::build(
                    &workspace,
                    skill_section,
                    instructions_section,
                    memory_section,
                    synthesis,
                    agent_synthesis,
                ),
                ..base
            },
        )
        // The same board the tool writes to, so what the model wrote on the
        // last iteration is what it reads on the next one.
        .with_plan(plan)
    }

    /// The open workspace's checkpoint logs, for the turn about to record into
    /// them.
    pub async fn checkpoints(&self) -> CheckpointStore {
        CheckpointStore::new(crate::sessions::checkpoints_dir(
            &self.workspace.read().await,
        ))
    }

    /// One named workspace's checkpoint logs.
    ///
    /// What anything *reading* a conversation's history wants, because a
    /// checkpoint log is keyed by the workspace the conversation was held in
    /// rather than by the one open now. Asked against the wrong workspace the
    /// log is simply not there, and the answer is an empty history for a
    /// conversation that rewrote half the project — which is what listing and
    /// rewinding both used to do the moment somebody switched folders.
    pub fn checkpoints_for(&self, workspace: &Path) -> CheckpointStore {
        CheckpointStore::new(crate::sessions::checkpoints_dir(workspace))
    }

    /// Reads one turn back to an agent that did not write it.
    ///
    /// Assembled here for the reason [`Self::build_agent`] is: this is the
    /// second place a provider, a registry and a tool context come together,
    /// and the two of them agreeing about what an agent may do is the whole
    /// value of there being one host. What differs is everything a review does
    /// not need — no checkpoint recorder, no background jobs, no per-turn tools
    /// — and the absences are the design. With no recorder the reviewer cannot
    /// produce a rewindable write even if its tool list were wrong, which is
    /// the belt beside [`crate::review`]'s braces.
    ///
    /// Config is *not* refreshed first, unlike a turn. A review is about a turn
    /// that has already run, and re-reading `AGENTS.md` on the way in would
    /// change nothing about it while costing a walk of the workspace.
    ///
    /// The answer comes back to the caller and goes nowhere near the
    /// transcript. See [`crate::review`] for why that is not an omission.
    pub async fn review_turn(
        &self,
        provider: Arc<dyn Provider>,
        model: &str,
        session_id: &str,
        turn: u32,
        cancel: CancellationToken,
    ) -> Result<crate::review::ReviewReport, String> {
        let workspace = self.workspace().await;
        let changes = self
            .checkpoints_for(&workspace)
            .changes(session_id, &workspace, turn)?;

        // The shared registry, which is the one a delegate gets: it holds no
        // `ask_user`, no chart tools, and no plan board, because those address
        // the person watching a conversation and a review is not one.
        let registry = self.registry.read().await.clone();

        crate::review::review(
            provider,
            model,
            registry,
            self.review_context(cancel.clone()).await,
            changes,
            turn,
            cancel,
        )
        .await
    }

    /// The context a review's tools run in.
    ///
    /// [`Self::tool_context`] with three things left off, each an absence with
    /// a reason. No checkpoint recorder: nothing here writes, and a recorder
    /// would open a turn on a conversation that is not having one. No jobs: a
    /// reviewer that started a background command would leave it running for a
    /// review that has ended. No command output directory: it has no
    /// `run_command` to cut the output of.
    ///
    /// Hooks are kept. A guard a delegate could route around is not a guard,
    /// and this is a delegate by every measure but the one that spawned it.
    pub(super) async fn review_context(&self, cancel: CancellationToken) -> ToolContext {
        let workspace = self.workspace.read().await.clone();
        let permissions = self.permissions.read().await.clone();
        // The loaded skills' own directories, so a reviewer following a
        // procedure's "see references/…" can read it. Read-only, and it widens
        // nothing that may be written.
        let roots = self.catalog.read().await.dirs();
        let hooks = self.hooks.read().await.clone();
        ToolContext::new(workspace, permissions, cancel)
            .with_readable_roots(roots)
            .with_hooks(hooks)
    }

    /// Where this workspace stands with git.
    ///
    /// Read on demand rather than cached: a user switches branches in a
    /// terminal beside this window, and a cached answer would be wrong exactly
    /// when it matters — the moment before someone commits a turn.
    pub async fn repo_status(&self) -> crate::git::RepoStatus {
        crate::git::Repo::status(&self.workspace.read().await.clone()).await
    }

    /// The branch this workspace is on, for the status and for stamping onto a
    /// new conversation.
    ///
    /// Read off disk rather than asked of git — see
    /// [`crate::git::branch_on_disk`] — because the status is pushed after
    /// nearly everything, and asking cost two git processes every time. Git is
    /// asked only when the disk does not settle it.
    pub async fn branch(&self) -> Option<String> {
        let workspace = self.workspace.read().await.clone();
        let on_disk = {
            let workspace = workspace.clone();
            tokio::task::spawn_blocking(move || crate::git::branch_on_disk(&workspace))
                .await
                .ok()
                .flatten()
        };
        match on_disk {
            Some(branch) => branch,
            None => crate::git::Repo::status(&workspace).await.branch,
        }
    }

    /// The hooks in force. Cloned rather than borrowed so a turn holds the set
    /// it started with, the same rule the agent roster follows.
    pub async fn hooks(&self) -> Arc<taurus_hooks::HookRunner> {
        self.hooks.read().await.clone()
    }

    /// Every hook that will run, for a listing.
    pub async fn hook_summaries(&self) -> Vec<taurus_hooks::HookSummary> {
        self.hooks.read().await.summaries()
    }

    /// The files hooks are read from, in precedence order.
    ///
    /// Answers "why is my hook not running" in the one case a listing cannot:
    /// in an untrusted workspace the project file is deliberately not among
    /// them, and a list that quietly omitted it would look like a bug.
    pub async fn hook_files(&self) -> Vec<PathBuf> {
        let workspace = self.workspace.read().await.clone();
        config::config_dirs(Some(&workspace))
            .iter()
            .map(|dir| taurus_hooks::config_file(dir))
            .collect()
    }

    pub async fn tool_context(&self, cancel: CancellationToken) -> ToolContext {
        let workspace = self.workspace.read().await.clone();
        // Where a command whose output had to be cut writes the whole of it.
        // Out of the project, keyed by workspace, beside the transcripts and
        // checkpoints it is the third kind of.
        let command_output = crate::sessions::output_dir(&workspace);
        // Read-only, and only what the session actually reaches for: the
        // skills it loaded, whose procedures point at their own bundled files
        // under the home directory, and the place a cut command's output was
        // written. Both are outside the workspace the guard otherwise confines
        // everything to, and neither widens what may be written.
        let mut readable = self.catalog.read().await.dirs();
        readable.push(command_output.clone());
        ToolContext::new(workspace, self.permissions.read().await.clone(), cancel)
            .with_readable_roots(readable)
            .with_command_output(command_output)
            // Carried on the context rather than looked up per call, so a
            // clone — which is how a sub-agent gets its context — goes through
            // the same hooks the parent does. A guard a delegate could route
            // around is not a guard.
            .with_hooks(self.hooks.read().await.clone())
            // The one thing on the context that is neither per turn nor per
            // call: a background command is read by the turns after the one
            // that started it, so a fresh set per turn would lose every one of
            // them.
            .with_jobs(self.jobs.clone())
            // The same for what earlier commands read of the workspace. Held
            // here rather than per turn, a turn's first command costs what its
            // second does.
            .with_sweep_cache(self.sweeps.clone())
    }

    /// Ends every background command, for a window closing.
    ///
    /// Public because the app is what knows the window is going: nothing in
    /// the OS tidies up a child that outlived the call that spawned it.
    pub fn stop_background(&self) {
        self.jobs.stop_all();
    }

    /// Every background command, for the window that draws them.
    ///
    /// The model reaches these through `check_command`; this is the other
    /// reader, and the two do not move each other's place in the output. See
    /// [`taurus_tools::Jobs`].
    pub fn jobs(&self) -> Vec<taurus_tools::BackgroundJob> {
        self.jobs.list()
    }

    /// What one background command has said after `cursor`.
    pub fn job_output(&self, id: u32, cursor: usize) -> Result<taurus_tools::JobOutput, String> {
        self.jobs.read(id, cursor)
    }

    /// Ends one background command, and waits for it to actually be gone.
    pub async fn stop_job(&self, id: u32) -> Result<String, String> {
        self.jobs.stop(id).await
    }

    pub async fn workspace(&self) -> PathBuf {
        self.workspace.read().await.clone()
    }

    /// Records the provider and model just used, in both layers.
    ///
    /// The workspace copy is what makes a repo reopen on the model it was last
    /// worked in; the global copy is the starting point for a workspace that
    /// has no memory of its own yet.
    pub async fn remember_session(&self, provider_id: &str, model: &str) {
        let workspace = self.workspace.read().await.clone();
        for (scope, dir) in [
            (Scope::Global, None),
            (Scope::Workspace, Some(workspace.as_path())),
        ] {
            config::edit_settings(scope, dir, |s| {
                s.last_provider = Some(provider_id.to_string());
                s.last_model = Some(model.to_string());
            });
        }

        let mut settings = self.settings.write().await;
        settings.last_provider = Some(provider_id.to_string());
        settings.last_model = Some(model.to_string());
    }

    /// This conversation's checklist, created on first use.
    pub(super) async fn plan_board(&self, session_id: &str) -> PlanBoard {
        if let Some(board) = self.plans.read().await.get(session_id) {
            return board.clone();
        }
        self.plans
            .write()
            .await
            .entry(session_id.to_string())
            .or_default()
            .clone()
    }

    /// Drops a conversation's checklist.
    ///
    /// Called when a session is deleted. Without it the map is the one thing in
    /// the host that only ever grows — a few hundred bytes per conversation
    /// opened, which is nothing until a long-running window has opened a
    /// thousand of them.
    pub async fn forget_plan(&self, session_id: &str) {
        self.plans.write().await.remove(session_id);
    }

    pub async fn permissions(&self) -> Arc<PermissionEngine> {
        self.permissions.read().await.clone()
    }
}
