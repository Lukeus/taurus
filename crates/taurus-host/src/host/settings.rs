//! Settings the UI writes, the themes, and the code index they configure.

use super::*;

impl Host {
    pub async fn settings(&self) -> Settings {
        self.settings.read().await.clone()
    }

    /// The active profile, for Settings › Profile.
    pub async fn profile(&self) -> profile::ProfileView {
        profile::ProfileView::new(profile::active_id(), self.profile.read().await.clone())
    }

    /// Saves the active profile and holds what was saved, so the next turn's
    /// prompt carries it. A save clears whatever the last reload said was
    /// wrong with the file, since the file is now the one this wrote.
    pub async fn save_profile(&self, edited: Profile) -> Result<profile::ProfileView, String> {
        let id = profile::active_id();
        let saved = off_runtime("saving the profile", move || profile::save(id, edited)).await?;
        *self.profile.write().await = saved.clone();
        self.replace_problems(ProblemSource::Profile, Vec::new())
            .await;
        Ok(profile::ProfileView::new(id, saved))
    }

    /// The global `search.json`, for the settings editor.
    pub fn global_search(&self) -> taurus_web::SearchFile {
        config::load_global_search()
    }

    /// Where each configured search backend's key comes from.
    ///
    /// The twin of [`Self::key_statuses`], and for the same reason: both
    /// frontends draw a list of these at once, and asking per backend would be
    /// one credential-store round trip per row.
    pub fn search_key_statuses(&self) -> Vec<(String, secrets::KeyStatus)> {
        config::load_global_search()
            .backends
            .iter()
            .map(|(id, entry)| {
                (
                    id.clone(),
                    config::search_key_status(id, entry.api_key_env.as_deref()),
                )
            })
            .collect()
    }

    /// Whether web search resolved to something that can actually run.
    ///
    /// Distinct from "a backend is selected": a selection with no key resolves
    /// to nothing, and the difference is what the settings screen shows.
    pub async fn search_active(&self) -> bool {
        self.registry.read().await.get("web_search").is_some()
    }

    /// Saves the global search layer and rebuilds, so turning search on
    /// registers its tools without a restart.
    ///
    /// The local half only, here and in the two key setters below: web search
    /// changes nothing an MCP server is running with.
    pub async fn set_search(&self, file: taurus_web::SearchFile) {
        config::save_search(&file);
        self.reload_local().await;
    }

    pub async fn set_search_key(&self, backend_id: &str, key: &str) -> Result<(), String> {
        let (id, key) = (config::search_key_id(backend_id), key.to_string());
        off_runtime("saving the key", move || secrets::store(&id, &key)).await?;
        // A saved key can be the thing that makes a selected backend resolve,
        // and the tools are only registered for one that does.
        self.reload_local().await;
        Ok(())
    }

    pub async fn clear_search_key(&self, backend_id: &str) -> Result<(), String> {
        let id = config::search_key_id(backend_id);
        off_runtime("removing the key", move || secrets::clear(&id)).await?;
        self.reload_local().await;
        Ok(())
    }

    /// Retunes one sub-agent's iteration limit, in place.
    ///
    /// Everything else about the agent is preserved, `model:` and `provider:`
    /// included — see [`taurus_agents::AgentDefinition::write_to`] for why that
    /// is not the same code path an approved proposal takes.
    ///
    /// A built-in has no file to edit, so changing one writes a user-tier
    /// override: the built-in stays as it shipped, and the copy shadows it
    /// everywhere. The path comes back so the caller can say which file now
    /// exists, because a control that silently creates one is a control that
    /// surprises whoever finds the file later.
    pub async fn set_agent_iterations(&self, name: &str, limit: u32) -> Result<String, String> {
        let limit = limit.clamp(1, taurus_agents::MAX_ITERATIONS_LIMIT);
        let workspace = self.workspace.read().await.clone();

        let mut definition = self
            .agents
            .read()
            .await
            .get(name)
            .ok_or_else(|| format!("no agent named '{name}'"))?
            .clone();
        // Where the agent editor's Edit writes, by the same rule, so the two
        // never disagree about which file an agent lives in. See
        // `crate::library::agent_edit_target`.
        let managed = match &definition.plugin {
            Some(plugin) => self.managed_plugins().await.contains(plugin),
            None => false,
        };
        let (path, forks) = crate::library::agent_edit_target(&definition, &workspace, managed);
        // A plugin's agent is named `plugin:agent` in the catalog and `agent`
        // in its file. Written back as loaded, the colon would land in the
        // file and the agent would never load again.
        definition.frontmatter.name = crate::library::own_name(&definition).to_string();

        if definition.frontmatter.max_iterations == limit && !forks {
            return Ok(path.display().to_string());
        }
        definition.frontmatter.max_iterations = limit;
        definition
            .write_to(&path)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;

        info!(agent = name, limit, path = %path.display(), "agent iteration limit changed");
        self.rescan_agents().await;
        Ok(path.display().to_string())
    }

    /// Sets how many model turns one message may take, for every workspace.
    ///
    /// Clamped on the way in as well as on the way out: `load_settings` brings a
    /// hand-edited number back into range, and doing it here too means the
    /// value written to the file is the value that will be used, rather than
    /// one that silently reads back as something else.
    ///
    /// No registry work, unlike the two toggles below — this changes a number
    /// the next turn reads, not which tools exist.
    pub async fn set_max_iterations(&self, limit: u32) -> Result<(), String> {
        let limit = limit.clamp(1, taurus_agents::MAX_ITERATIONS_LIMIT);
        self.edit_global_setting(|s| s.max_iterations = Some(limit))
            .await
    }

    /// How many sub-agents one turn may run at once, for every workspace. See
    /// [`crate::config::Settings::max_concurrent_agents`]. Clamped on the way
    /// in, like the iteration ceiling.
    pub async fn set_max_concurrent_agents(&self, count: u32) -> Result<(), String> {
        let count = count.clamp(1, crate::MAX_CONCURRENT_SUBAGENTS_LIMIT);
        self.edit_global_setting(|s| s.max_concurrent_agents = Some(count))
            .await
    }

    /// Which tools to leave out, for every workspace. See
    /// [`crate::config::Settings::disabled_tools`].
    ///
    /// Only writes. The registry is rebuilt by a reload, which the caller runs
    /// because it also has MCP tools to put back, and a reload is the one path
    /// that already knows how to do both.
    pub async fn set_disabled_tools(&self, mut names: Vec<String>) -> Result<(), String> {
        names.retain(|n| !n.trim().is_empty());
        names.sort();
        names.dedup();
        self.edit_global_setting(|s| {
            s.disabled_tools = (!names.is_empty()).then_some(names);
        })
        .await
    }

    /// Where traces are sent. Empty sends them nowhere. Read when the app
    /// starts, so this takes effect on the next launch; see
    /// `taurus_telemetry::install`.
    ///
    /// Global only, like the exporter that reads it: a trace destination is a
    /// property of the machine.
    pub async fn set_otlp_endpoint(&self, endpoint: &str) -> Result<(), String> {
        let endpoint = endpoint.trim().to_string();
        let http = endpoint.starts_with("http://") || endpoint.starts_with("https://");
        if !(endpoint.is_empty() || http) {
            return Err(format!(
                "\"{endpoint}\" isn't an OTLP/HTTP address. It starts with http:// or \
                 https://, like http://localhost:4318."
            ));
        }
        self.edit_global_setting(|s| s.otlp_endpoint = Some(endpoint))
            .await
    }

    /// Whether a turn that finishes while you're elsewhere sends a system
    /// notification. Global: it's about the person, not the project.
    pub async fn set_notify_when_done(&self, on: bool) -> Result<(), String> {
        self.edit_global_setting(|s| s.notify_when_done = Some(on))
            .await
    }

    /// How large the interface is drawn, in percent. Clamped on the way in, so
    /// what's written is what's used. The caller applies it to the window.
    pub async fn set_ui_scale(&self, percent: u32) -> Result<u32, String> {
        let percent = percent.clamp(config::MIN_UI_SCALE, config::MAX_UI_SCALE);
        self.edit_global_setting(|s| s.ui_scale = Some(percent))
            .await?;
        Ok(percent)
    }

    /// The program the terminal dock starts. Empty is the login shell.
    ///
    /// Checked here rather than when a terminal opens, so a typo is said
    /// while the field is still on screen. Applies to the next shell opened;
    /// one already running keeps going.
    pub async fn set_terminal_shell(&self, program: &str) -> Result<(), String> {
        let program = program.trim().to_string();
        if !program.is_empty() && taurus_tools::login_path::which(&program).is_none() {
            return Err(if program.contains('/') || program.contains('\\') {
                format!("{program} isn't a program this app can run.")
            } else {
                format!(
                    "No program called \"{program}\" is on this app's PATH. Give its full \
                     path instead, like /opt/homebrew/bin/{program}."
                )
            });
        }
        self.edit_global_setting(|s| s.terminal_shell = Some(program))
            .await
    }

    /// The terminal's font family and size. An empty family is the app's own
    /// monospace stack. The size is clamped on the way in.
    pub async fn set_terminal_font(&self, family: &str, size: u32) -> Result<(), String> {
        let family = family.trim().to_string();
        let size = size.clamp(
            config::MIN_TERMINAL_FONT_SIZE,
            config::MAX_TERMINAL_FONT_SIZE,
        );
        self.edit_global_setting(|s| {
            s.terminal_font = Some(family);
            s.terminal_font_size = Some(size);
        })
        .await
    }

    /// Whether exported traces may carry the messages themselves. Read at the
    /// start of each turn, so it takes effect on the next one.
    pub async fn set_otlp_capture_content(&self, capture: bool) -> Result<(), String> {
        self.edit_global_setting(|s| s.otlp_capture_content = Some(capture))
            .await
    }

    /// How much of a model's window a turn fills before it makes room. See
    /// [`crate::config::Settings::context_limit`]. Clamped on the way in, like
    /// the iteration ceiling, so what is written is what will be used.
    pub async fn set_context_limit(&self, limit: u32) -> Result<(), String> {
        let limit = crate::config::clamp_context_limit(limit);
        self.edit_global_setting(|s| s.context_limit = Some(limit))
            .await
    }

    /// Writes one global setting and reloads the resolved ones.
    ///
    /// What every setter the UI calls does, and the reason it is one function:
    /// the global file is the layer the UI edits, and the workspace's layer
    /// still has to be applied over it before anything reads the result.
    ///
    /// Says why when the write didn't land — a global file that won't parse,
    /// a directory that can't be written — rather than reloading as if it had,
    /// which left a switch that moved and then moved back.
    pub(super) async fn edit_global_setting(
        &self,
        edit: impl FnOnce(&mut config::StoredSettings),
    ) -> Result<(), String> {
        config::edit_settings(Scope::Global, None, edit)?;
        let workspace = self.workspace.read().await.clone();
        *self.settings.write().await = config::load_settings(Some(&workspace));
        Ok(())
    }

    /// Toggles skill synthesis for every workspace.
    ///
    /// A project that wants it off regardless can say so in its own
    /// `.taurus/settings.json`, which this will not overwrite.
    pub async fn set_skill_synthesis(&self, enabled: bool) -> Result<(), String> {
        self.edit_global_setting(|s| s.skill_synthesis_enabled = Some(enabled))
            .await?;

        // The tool follows the setting rather than waiting for a reload, so
        // turning synthesis off stops paying for its schema on the next request
        // instead of the next restart. Rebuilding the whole registry here would
        // also drop every MCP connection, which a checkbox has no business
        // doing.
        let resolved = self.settings.read().await.clone();
        let mut registry = self.registry.write().await;
        registry.remove(taurus_skills::PROPOSE_TOOL);
        if resolved.skill_synthesis_enabled
            && !resolved
                .disabled_tools
                .iter()
                .any(|d| d == taurus_skills::PROPOSE_TOOL)
        {
            registry.register(Arc::new(taurus_skills::ProposeSkill::new(
                self.catalog.clone(),
                self.proposals.clone(),
            )));
        }
        Ok(())
    }

    /// Turns the terminal dock's shell integration on or off, everywhere.
    ///
    /// Nothing else follows it: a shell already running keeps the hooks it
    /// started with, and the next one opened reads this.
    pub async fn set_context_strategy(
        &self,
        strategy: taurus_core::ContextStrategy,
    ) -> Result<(), String> {
        self.edit_global_setting(|s| s.context_strategy = Some(strategy))
            .await
    }

    pub async fn set_shell_integration(&self, enabled: bool) -> Result<(), String> {
        self.edit_global_setting(|s| s.shell_integration = Some(enabled))
            .await
    }

    /// Whether a shell opened now should get the integration.
    pub async fn shell_integration(&self) -> bool {
        self.settings.read().await.shell_integration
    }

    /// Toggles sub-agent synthesis for every workspace.
    ///
    /// The twin of [`Host::set_skill_synthesis`], down to not rebuilding the
    /// registry: a checkbox has no business dropping every MCP connection.
    pub async fn set_agent_synthesis(&self, enabled: bool) -> Result<(), String> {
        self.edit_global_setting(|s| s.agent_synthesis_enabled = Some(enabled))
            .await?;

        let resolved = self.settings.read().await.clone();
        let mut registry = self.registry.write().await;
        registry.remove(taurus_core::PROPOSE_AGENT_TOOL);
        if resolved.agent_synthesis_enabled
            && !resolved
                .disabled_tools
                .iter()
                .any(|d| d == taurus_core::PROPOSE_AGENT_TOOL)
        {
            registry.register(Arc::new(ProposeAgent::new(
                self.agents.clone(),
                self.registry.clone(),
                self.agent_proposals.clone(),
            )));
        }
        Ok(())
    }

    /// Sets the palette for every workspace.
    ///
    /// Global only, like every other edit from the UI: a theme is a property of
    /// the person looking at the screen, not of the project on it, and writing
    /// it into a workspace file would hand one repo the power to decide how the
    /// app looks everywhere it is opened.
    pub async fn set_theme(&self, theme: Theme) -> Result<(), String> {
        self.edit_global_setting(|s| s.theme = Some(theme)).await
    }

    /// Picks the custom theme painting over that palette, or none of them.
    ///
    /// Global for the same reason [`Host::set_theme`] is: which brand the
    /// window wears is a property of the person looking at it. A repository
    /// can still ship one and name it in its own `settings.json` by hand —
    /// that is a layer, and a hand-edited layer is a different thing from a
    /// click in the app quietly writing into somebody's project.
    pub async fn set_theme_id(&self, id: String) -> Result<(), String> {
        self.edit_global_setting(|s| s.theme_id = Some(id)).await
    }

    /// The custom theme in force, if there is one.
    ///
    /// This is what rides on every status the window is pushed, so it reads
    /// the one file rather than the directory — see [`theme::load_theme`] —
    /// and only when that file, the layer above it, or its logo has moved since
    /// the last read. See [`Self::theme_seen`]. Its problems are recorded on
    /// the way past, which is what makes a theme that was deleted out from
    /// under the setting say so instead of the app just quietly losing its
    /// brand.
    pub async fn active_theme(&self) -> Option<CustomTheme> {
        let id = self.settings.read().await.theme_id.clone();
        let workspace = self.workspace.read().await.clone();
        let reading = crate::trust::for_reading(Some(&workspace)).map(Path::to_path_buf);

        // The one held, if nothing it was read from has moved: the same id,
        // the same trusted layers, and every file as it was.
        let held = self
            .theme_seen
            .read()
            .await
            .as_ref()
            .filter(|held| {
                held.id == id && held.reading == reading && held.seen == held.seen.refreshed()
            })
            .map(|held| (held.theme.clone(), held.problems.clone()));
        let (theme, problems) = match held {
            Some(held) => held,
            None => {
                let (theme, problems, watched) = theme::load_theme_watched(Some(&workspace), &id);
                *self.theme_seen.write().await = Some(ResolvedTheme {
                    id,
                    reading,
                    seen: Freshness::of_files(watched.iter().map(PathBuf::as_path)),
                    theme: theme.clone(),
                    problems: problems.clone(),
                });
                (theme, problems)
            }
        };
        // Restated when the theme came from the cache as well: a reload
        // replaces the problem list, and the theme's would otherwise vanish
        // until its file next moved.
        self.replace_problems(
            ProblemSource::Themes,
            Problem::tag(ProblemSource::Themes, problems),
        )
        .await;
        theme
    }

    /// Every custom theme both layers offer, and what is wrong with the rest.
    ///
    /// Re-read on every call rather than cached. A theme is a file people edit
    /// in another window — that is the point of it being a file — so a picker
    /// showing the set as it was at startup is a picker that cannot show the
    /// feature working. It is a directory listing and a handful of small JSON
    /// files; the cache would cost more to invalidate correctly than the read
    /// costs to repeat.
    pub async fn themes(&self) -> Vec<CustomTheme> {
        let workspace = self.workspace.read().await.clone();
        let (themes, problems) = theme::load_themes(Some(&workspace));
        self.replace_problems(
            ProblemSource::Themes,
            Problem::tag(ProblemSource::Themes, problems),
        )
        .await;
        themes
    }

    /// Writes a theme into a layer and returns where it landed.
    pub async fn save_theme(
        &self,
        scope: Scope,
        id: &str,
        file: &theme::ThemeFile,
    ) -> Result<String, String> {
        let workspace = self.workspace.read().await.clone();
        let saved = theme::save_theme(scope, Some(&workspace), id, file)?;
        // Forgotten outright rather than left to the file's stamp: the editor
        // saves on every change, and two saves of the same length inside one
        // tick of the filesystem's clock would look like no change at all.
        *self.theme_seen.write().await = None;
        Ok(saved.display().to_string())
    }

    /// Removes a theme file.
    ///
    /// The setting is cleared alongside it when it was the one in force, so
    /// deleting the theme you are looking at puts the window back on the
    /// built-in palette rather than leaving `theme_id` naming a file that is
    /// no longer there.
    pub async fn delete_theme(&self, scope: Scope, id: &str) -> Result<(), String> {
        let workspace = self.workspace.read().await.clone();
        theme::delete_theme(scope, Some(&workspace), id)?;
        if self.settings.read().await.theme_id == id {
            self.set_theme_id(String::new()).await?;
        }
        Ok(())
    }

    /// Creates a layer's themes directory and returns it, for the row that
    /// offers to open the folder.
    pub async fn themes_dir(&self, scope: Scope) -> Result<String, String> {
        let workspace = self.workspace.read().await.clone();
        theme::ensure_themes_dir(scope, Some(&workspace)).map(|p| p.display().to_string())
    }

    /// Which provider serves the embedding model.
    ///
    /// What the config named, if it named one. Otherwise the one the
    /// conversation is on, falling back to the first configured: an embedding
    /// model lives on the same server as the chat model in every local setup,
    /// and a second provider entry naming the same machine would be one more
    /// thing to keep in step.
    ///
    /// The configured case exists because that stopped covering everything.
    /// Anthropic has no embedding endpoint and points at Voyage instead, so
    /// somebody chatting to Claude has to be able to index somewhere else
    /// without switching the conversation to do it.
    ///
    /// A method rather than an expression at each of its two call sites because
    /// the first version was written twice and the copy reached for
    /// `blocking_read` inside an async fn — which tokio answers by panicking,
    /// on the one path nobody exercises: a machine with no remembered provider.
    pub(super) async fn embedding_provider_id(&self) -> Option<String> {
        let settings = self.settings.read().await;
        let named = settings.embedding_provider.trim();
        if !named.is_empty() {
            return Some(named.to_string());
        }
        if let Some(id) = settings.last_provider.clone() {
            return Some(id);
        }
        drop(settings);
        self.providers.read().await.first().map(|p| p.id.clone())
    }

    /// The reranking provider and model, when both are configured.
    ///
    /// `Ok(None)` is the ordinary case: nothing is configured and the
    /// similarity order stands. `Err` means something *was* configured and
    /// could not be resolved, which is worth telling the user about — a
    /// reranker that silently never runs is indistinguishable from one that is
    /// running and not helping, and those two want opposite fixes.
    ///
    /// `embedding` is the provider the index already resolved, used when no
    /// separate one is named. Passed in rather than looked up again so the two
    /// cannot drift apart on a machine whose configured provider changed
    /// between the two lookups.
    pub(super) async fn rerank_for(
        &self,
        embedding: &Arc<dyn taurus_provider::Provider>,
    ) -> Result<Option<(Arc<dyn taurus_provider::Provider>, String)>, String> {
        let settings = self.settings.read().await;
        let model = settings.rerank_model.trim().to_string();
        let named = settings.rerank_provider.trim().to_string();
        drop(settings);

        if model.is_empty() {
            return Ok(None);
        }
        if named.is_empty() {
            return Ok(Some((Arc::clone(embedding), model)));
        }
        // Deferred, for the reason the embedding provider is: this is wired in
        // on every reload, and building a provider reads its key.
        match self.deferred_provider(&named).await {
            Some(provider) => Ok(Some((provider, model))),
            None => Err(format!(
                "reranking is configured on '{named}' but no provider configured with id \
                 '{named}'. Search still works; results are ordered by similarity alone until \
                 this resolves."
            )),
        }
    }

    /// Which embedding model semantic search runs on, and which backend serves
    /// it. An empty model means off; an empty provider means the one the
    /// conversation is on.
    ///
    /// Both at once because they are one decision, the same as reranking: a
    /// model saved without a provider embeds on whichever backend the
    /// conversation happens to be using, and for somebody chatting to Claude
    /// that is a backend with no embedding endpoint at all.
    ///
    /// Global only. It names a model on the machine's own server, which is a
    /// property of the machine rather than of any one project.
    pub async fn set_embedding_model(&self, model: &str, provider: &str) -> Result<(), String> {
        let model = model.trim().to_string();
        let provider = provider.trim().to_string();
        self.edit_global_setting(|s| {
            s.embedding_model = Some(model);
            s.embedding_provider = Some(provider);
        })
        .await
    }

    /// Which reranking model reorders search results, and which provider
    /// serves it. An empty model turns the stage off.
    ///
    /// Global, like the embedding model beside it and for the same reason: it
    /// names a model on a server this machine can reach, which is a property of
    /// the machine rather than of any project opened on it.
    ///
    /// Both at once because they are one decision. Saved separately, a model
    /// with no provider yet would spend one save reranking on whichever backend
    /// the conversation happened to be on — which for the common Ollama setup
    /// is a backend that cannot rerank at all, and so a round trip that fails
    /// on every search until the second field lands.
    pub async fn set_rerank(&self, model: &str, provider: &str) -> Result<(), String> {
        let model = model.trim().to_string();
        let provider = provider.trim().to_string();
        self.edit_global_setting(|s| {
            s.rerank_model = Some(model);
            s.rerank_provider = Some(provider);
        })
        .await
    }

    /// Which speech model the mic transcribes with.
    ///
    /// Global, because the model is a file on this machine: a project can't
    /// carry it, and a project setting naming one that isn't downloaded would
    /// turn the mic into a download prompt in that one folder. The caller
    /// checks the name against the catalog; this crate doesn't know it.
    pub async fn set_voice_model(&self, model: &str) -> Result<(), String> {
        let model = model.trim().to_string();
        self.edit_global_setting(|s| s.voice_model = Some(model))
            .await
    }

    /// How replies are read aloud.
    ///
    /// Global, like the speech model: which voice somebody wants to hear is
    /// theirs, not the project's. A workspace file can still set it by hand.
    pub async fn set_speech(&self, speech: config::SpeechSettings) -> Result<(), String> {
        self.edit_global_setting(|s| s.speech = Some(speech)).await
    }

    /// Where each speech service's key is coming from.
    pub async fn speech_key_statuses(&self) -> Vec<(config::SpeechEngine, secrets::KeyStatus)> {
        let speech = self.settings().await.speech;
        // On a blocking thread, like `key_statuses`: each status is a read of
        // the OS keychain, which with a locked keychain waits on its dialog.
        let statuses = [
            (
                config::SpeechEngine::ElevenLabs,
                speech.elevenlabs.api_key_env,
            ),
            (config::SpeechEngine::OpenAi, speech.openai.api_key_env),
        ]
        .into_iter()
        .map(|(engine, variable)| {
            let status = secrets::status(&config::speech_key_id(engine), variable.as_deref());
            (engine, status)
        });
        off_runtime_or_default(move || statuses.collect()).await
    }

    /// The key a speech service will be sent, honoring the environment over
    /// the store. Reads the credential store, which can block on a dialog:
    /// call it off the async runtime.
    pub fn speech_key(engine: config::SpeechEngine, variable: Option<&str>) -> Option<String> {
        secrets::resolve(&config::speech_key_id(engine), variable)
    }

    pub fn set_speech_key(engine: config::SpeechEngine, key: &str) -> Result<(), String> {
        secrets::store(&config::speech_key_id(engine), key)
    }

    pub fn clear_speech_key(engine: config::SpeechEngine) -> Result<(), String> {
        secrets::clear(&config::speech_key_id(engine))
    }

    /// Brings this workspace's semantic index up to date, outside any turn.
    ///
    /// The first index of a repository takes the better part of a minute, and
    /// until this existed the only way to pay that was to be halfway through a
    /// turn when the model first reached for `search_code` — a turn that then
    /// sat on an unreturned tool call for the whole of it. Run here, the cost
    /// is paid when someone chose to pay it, against a progress bar, with a
    /// Stop that stops indexing rather than a conversation.
    ///
    /// It is the same `refresh` the tool calls, deliberately: an index built
    /// here and an index brought up to date by a search have to be the same
    /// thing, or one of the two paths is quietly writing a second format.
    pub async fn build_index(
        &self,
        cancel: CancellationToken,
        progress: Option<&dyn taurus_index::IndexProgress>,
    ) -> Result<String, String> {
        let model = self
            .settings
            .read()
            .await
            .embedding_model
            .trim()
            .to_string();
        if model.is_empty() {
            return Err(
                "No embedding model is set, so there is no index to build. Name one under \
                 Settings → Search."
                    .into(),
            );
        }

        let id = self
            .embedding_provider_id()
            .await
            .ok_or("No provider is configured, so there is nothing to embed with.")?;
        let provider = self.provider(&id).await?;

        let workspace = self.workspace.read().await.clone();
        let index = taurus_index::Index::new(
            taurus_index::index_dir(
                &config::home_dir(),
                &crate::sessions::workspace_key(&workspace),
            ),
            &workspace,
        );

        // Stops the warm-up if one is running, rather than embedding the same
        // passages beside it while the person watching this progress bar waits.
        let ticket = self.indexing.take_over(&cancel);
        let refreshed =
            taurus_index::refresh(&index, &workspace, &provider, &model, &cancel, progress).await;
        self.indexing.finished(ticket);
        let (_, report) = refreshed?;
        Ok(report.summary())
    }

    /// Starts bringing this workspace's index up to date, without waiting.
    ///
    /// The first index of a repository is the better part of a minute, and
    /// until this existed the only ways to pay it were a Settings button
    /// somebody had to know about and a `search_code` call that stalled the
    /// turn it was made in. Started with the turn instead, the model's first
    /// search lands on an index that has been building since the message was
    /// sent — and if it lands early, the tool takes the refresh over and
    /// finishes it with progress in the transcript rather than starting again.
    ///
    /// Does nothing without an embedding model, which is the same switch that
    /// decides whether `search_code` exists at all: nobody's machine embeds a
    /// repository because they opened it.
    ///
    /// Nothing waits on the result. A refresh that fails here is a refresh the
    /// search would have failed at too, and it says so there, to the reader who
    /// asked a question — rather than here, to nobody.
    pub async fn warm_index(&self) {
        if self.indexing.busy() {
            return;
        }
        let model = self
            .settings
            .read()
            .await
            .embedding_model
            .trim()
            .to_string();
        if model.is_empty() {
            return;
        }
        let Some(id) = self.embedding_provider_id().await else {
            return;
        };
        let Ok(provider) = self.provider(&id).await else {
            return;
        };

        let workspace = self.workspace.read().await.clone();
        let index = taurus_index::Index::new(
            taurus_index::index_dir(
                &config::home_dir(),
                &crate::sessions::workspace_key(&workspace),
            ),
            &workspace,
        );

        // Registered before the task starts rather than inside it, so two turns
        // in quick succession cannot both find nothing running.
        let cancel = CancellationToken::new();
        let ticket = self.indexing.take_over(&cancel);
        let indexing = self.indexing.clone();
        tokio::spawn(async move {
            match taurus_index::refresh(&index, &workspace, &provider, &model, &cancel, None).await
            {
                Ok((_, report)) => tracing::debug!(summary = %report.summary(), "warmed the index"),
                Err(e) => tracing::debug!(error = %e, "the index warm-up stopped"),
            }
            indexing.finished(ticket);
        });
    }
}
