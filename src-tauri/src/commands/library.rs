//! Skills, agents, the tools they may use, and the proposals the model makes.

use super::*;

#[tauri::command]
pub async fn list_permission_rules(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<AllowedRule>> {
    Ok(state.host.permissions().await.allowed_rules().await)
}

#[tauri::command]
pub async fn revoke_permission_rule(
    state: State<'_, Arc<AppState>>,
    rule: String,
    scope: Scope,
) -> CmdResult<()> {
    state.host.permissions().await.revoke(&rule, scope).await;
    Ok(())
}

/// What a conversation has allowed once and could make standing. Asked when
/// it is set to unattended, which is when those are about to be refused. See
/// `PermissionEngine::allowed_once`.
#[tauri::command]
pub async fn once_allowed_rules(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> CmdResult<Vec<String>> {
    Ok(state
        .host
        .permissions()
        .await
        .allowed_once(&session_id)
        .await)
}

/// Makes rules standing for this workspace, as "Allow always" on each prompt
/// would have. See `PermissionEngine::grant`.
#[tauri::command]
pub async fn grant_permission_rules(
    state: State<'_, Arc<AppState>>,
    rules: Vec<String>,
) -> CmdResult<()> {
    state.host.permissions().await.grant(&rules).await;
    info!(
        count = rules.len(),
        "permission rules granted for the workspace"
    );
    Ok(())
}

#[tauri::command]
pub async fn list_skills(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<SkillSummary>> {
    // Rescanned first, the same as `list_agents`: the whole authoring surface
    // for a skill is a text editor and a folder, so a drawer showing the
    // catalog as it was at startup is not showing the feature working.
    //
    // The rail carries the count beside the drawer that lists them, and the two
    // disagreeing is worse than either being slightly late — so a rescan that
    // moved the count, or a problem, pushes a status. One that found the same
    // library does not: opening a drawer is not a reason to rebuild all of it.
    if state.host.rescan_skills().await {
        emit_status(&state).await;
    }
    Ok(state.host.skills().await)
}

/// The standing brief in force, in the order it reaches the prompt.
///
/// Shown beside the skill library because it belongs to the same question —
/// what is in the model's context before this conversation started — and
/// because a file being read silently is the kind of thing that makes
/// behaviour inexplicable. It is also where `ProblemSource::Instructions`
/// points, so a brief that did not load whole has somewhere to say so.
#[tauri::command]
pub async fn list_instructions(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<taurus_host::Instructions>> {
    Ok(state.host.instructions().await)
}

/// Skills and sub-agents the user can run as `/name`, for completion in the
/// composer.
///
/// A separate call from [`list_skills`] and [`list_agents`] rather than a merge
/// in the UI: which of either is user-invocable is the harness's answer to
/// give, and a composer offering one it would then refuse is a dead end typed
/// in full.
#[tauri::command]
pub async fn list_commands(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<taurus_host::CommandSummary>> {
    Ok(state.host.commands().await)
}

/// The sub-agent roster, rescanned from disk first.
///
/// Rescanning here rather than returning the cached catalog is what makes the
/// drawer show the feature working: the whole authoring surface is a text
/// editor, so a list assembled at startup is stale by the time anyone opens it.
#[tauri::command]
pub async fn list_agents(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<AgentSummary>> {
    // Same rule the skills listing follows: the rail's count and the drawer's
    // list are two views of one scan and must not disagree, and a scan that
    // moved neither has nothing to push.
    if state.host.rescan_agents().await {
        emit_status(&state).await;
    }
    Ok(state.host.agents().await)
}

/// Every tool this session has, for the editor's tool picker.
///
/// The live registry rather than a compiled-in list, so a skill or MCP tool
/// approved earlier in the session is offered — and so an agent cannot be
/// scoped to a tool that would be refused the moment it was saved.
#[tauri::command]
pub async fn list_tools(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<String>> {
    Ok(state.host.tool_names().await)
}

/// Saves an agent written in the editor.
///
/// The same path an approved proposal takes — same validation, same writer,
/// same rescan — because a hand-written agent and a generated one are the same
/// file, and two writers would drift.
///
/// Unlike an approved proposal, whose card says before anyone approves it that
/// it replaces an agent of the same name, the editor is for making a new one,
/// and refuses a file already there. To change an agent, open it.
///
/// `plugin` saves it into a plugin you made instead of `target`.
#[tauri::command]
pub async fn save_agent(
    state: State<'_, Arc<AppState>>,
    draft: AgentProposal,
    target: AgentSaveTarget,
    plugin: Option<String>,
) -> CmdResult<String> {
    let root = match plugin.as_deref() {
        None => validated_agent_root(&state, &draft, target).await?,
        Some(plugin) => {
            validated_agent_root(&state, &draft, target).await?;
            state
                .host
                .agent_save_root(SaveTarget::User, Some(plugin))
                .await?
        }
    };
    let file = root.join(format!("{}.md", draft.name));
    if file.exists() {
        return Err(format!(
            "an agent file named {}.md is already at {}. Open that agent to change it, or \
             choose another name.",
            draft.name,
            root.display()
        ));
    }
    let path = save_agent_file(&draft, &root).map_err(|e| format!("could not save agent: {e}"))?;
    info!(agent = %draft.name, path = %path.display(), "agent saved from the editor");

    state.host.rescan_agents().await;
    emit_status(&state).await;
    Ok(path.display().to_string())
}

/// What the model is told to produce for the editor's Generate button.
///
/// Named fields rather than "write an agent file": the editor owns the format,
/// and a model asked for YAML frontmatter returns YAML that is *nearly* right
/// often enough to matter. JSON it can be checked against, and every field
/// lands in a box the user can correct.
///
/// Built rather than declared, so the range it quotes is the range the loader
/// actually enforces. Stated as a literal it once drifted from the ceiling the
/// moment that ceiling moved, and a model told the wrong range returns drafts
/// that are silently clamped on the way in.
pub(super) fn draft_system() -> String {
    format!(
        "\
You draft a sub-agent definition for a coding agent, and reply with JSON only — \
no prose, no markdown fence.

Fields:
- name: kebab-case, specific, e.g. \"migration-checker\"
- description: one sentence under {DESCRIPTION_LIMIT} characters saying when to \
delegate here. This is the only text the caller sees when choosing, so describe \
the job.
- tools: an array chosen ONLY from the allowed list you are given, or null to \
inherit the caller's tools. Pick the narrowest set that can do the work.
- max_iterations: 1 to {MAX_ITERATIONS_LIMIT}. Around 20 for work that only \
reads, 25 if it writes. Reach past 50 only for work that genuinely cannot \
finish in fewer rounds.
- prompt: the agent's system prompt. It shares none of the caller's context and \
cannot ask questions, so say what to do, what not to do, and what to report \
back. Several sentences."
    )
}

/// Where an agent draft is filed in [`AppState::stoppable`]. One key, because
/// there is one editor.
pub(super) const AGENT_DRAFT: &str = "agent-draft";

/// Ends a draft [`generate_agent`] is still waiting on. Safe to call when none
/// is running.
#[tauri::command]
pub fn stop_agent_draft(state: State<'_, Arc<AppState>>) {
    state.stoppable.stop(AGENT_DRAFT);
}

/// Drafts an agent from a description, for the editor to fill in.
///
/// A one-shot completion rather than a turn: there are no tools to call and
/// nothing to undo, and running it through the agent loop would put a draft
/// nobody asked for into the transcript.
///
/// Whatever comes back is a starting point, not a result — it lands in the
/// editor's fields, and the user is the one who saves it. So this repairs what
/// it can (an out-of-range iteration count, a tool the session lacks) rather
/// than refusing a draft over something the user can see and fix.
#[tauri::command]
pub async fn generate_agent(
    state: State<'_, Arc<AppState>>,
    description: String,
    provider_id: String,
    model: String,
) -> CmdResult<AgentProposal> {
    if description.trim().is_empty() {
        return Err("describe what the agent should do first".into());
    }

    let available: Vec<String> = state
        .host
        .registry()
        .read()
        .await
        .names()
        .map(str::to_string)
        .collect();

    let prompt = format!(
        "Draft a sub-agent for: {}\n\nAllowed tools: {}",
        description.trim(),
        available.join(", ")
    );
    let text = one_shot(
        &state,
        AGENT_DRAFT,
        &provider_id,
        &model,
        draft_system(),
        prompt,
    )
    .await?;
    let json = extract_json(&text).ok_or_else(|| {
        format!(
            "{model} did not answer with JSON. It said: {}",
            brief(&text)
        )
    })?;
    let drafted: DraftedAgent = serde_json::from_str(json)
        .map_err(|e| format!("{model} answered with JSON that does not fit an agent: {e}"))?;

    let mut proposal = AgentProposal::new(
        drafted.name.trim(),
        drafted.description.trim(),
        drafted.prompt.trim(),
    );
    proposal.max_iterations = drafted
        .max_iterations
        .unwrap_or(20)
        .clamp(1, MAX_ITERATIONS_LIMIT);
    // Silently dropped rather than refused: a model naming a tool that does not
    // exist here has still drafted a usable agent, and the picker below shows
    // exactly what survived.
    proposal.tools = drafted.tools.map(|tools| {
        tools
            .into_iter()
            .filter(|tool| available.contains(tool))
            .collect()
    });
    // An empty list means "no tools" to the loader and "everything" to nobody.
    // If filtering emptied it, inheriting is the honest reading of a draft that
    // named only tools this session lacks.
    if proposal.tools.as_ref().is_some_and(Vec::is_empty) {
        proposal.tools = None;
    }

    info!(agent = %proposal.name, "agent drafted");
    Ok(proposal)
}

/// One completion with no tools, for an editor's Generate button. Answers with
/// the text, or says it was stopped.
///
/// Filed in [`AppState::stoppable`] under `key`, where the editor can reach it.
/// A draft on a local model takes minutes, and closing the editor has to be
/// able to end one.
async fn one_shot(
    state: &AppState,
    key: &str,
    provider_id: &str,
    model: &str,
    system: String,
    prompt: String,
) -> CmdResult<String> {
    let provider = state.host.provider(provider_id).await?;
    let mut request = ChatRequest::new(model, vec![Message::user(prompt)]);
    request.system = Some(system);

    let running = state.stoppable.start(key);
    let cancel = running.cancel.clone();
    let (tx, mut rx) = mpsc::channel(64);
    let handle = tokio::spawn(async move { provider.stream(request, tx, cancel).await });

    let mut acc = StreamAccumulator::new();
    while let Some(event) = rx.recv().await {
        acc.push(event);
    }
    let stop = handle
        .await
        .map_err(|e| format!("the draft did not finish: {e}"))?
        .map_err(|e| format!("could not reach {provider_id}: {e}"))?;
    // Before the text is read: what a stopped draft has said so far is not an
    // answer, and reading it would report a model that "did not answer with
    // JSON" to someone who pressed Stop.
    if stop == StopReason::Canceled {
        return Err("Drafting stopped.".into());
    }
    Ok(acc.finish().0.text())
}

#[derive(Deserialize)]
pub(super) struct DraftedAgent {
    name: String,
    description: String,
    prompt: String,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default)]
    max_iterations: Option<u32>,
}

/// The first JSON object in a reply.
///
/// Models wrap JSON in prose and markdown fences however often they are asked
/// not to, and a draft thrown away over a fence is a round trip spent on
/// punctuation. Brace-counting rather than a regex because the prompt itself
/// contains braces.
pub(super) fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            match ch {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + offset + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Enough of a reply to recognize it, for an error message.
pub(super) fn brief(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.chars().count() > 160 {
        true => format!("{}…", trimmed.chars().take(160).collect::<String>()),
        false => trimmed.to_string(),
    }
}

/// What the roster costs on every request, in characters. Shown beside it,
/// because an expense nobody can see is one nobody chose.
#[tauri::command]
pub async fn agent_roster_cost(state: State<'_, Arc<AppState>>) -> CmdResult<usize> {
    Ok(state.host.roster_cost().await)
}

#[tauri::command]
pub async fn list_proposals(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<SkillProposal>> {
    Ok(state
        .pending_proposals
        .iter()
        .map(|e| e.value().clone())
        .collect())
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct ProposalResponse {
    pub id: String,
    pub approve: bool,
    /// Where to save it. Ignored when rejecting.
    #[serde(default)]
    pub target: Option<SaveTarget>,
    /// The user's edits, if they changed anything in the review card.
    #[serde(default)]
    pub edited: Option<SkillProposal>,
}

#[tauri::command]
pub async fn respond_skill_proposal(
    state: State<'_, Arc<AppState>>,
    response: ProposalResponse,
) -> CmdResult<Option<String>> {
    let Some((_, original)) = state.pending_proposals.remove(&response.id) else {
        return Err(format!("no pending proposal '{}'", response.id));
    };

    if !response.approve {
        info!(skill = %original.name, "skill proposal rejected");
        return Ok(None);
    }

    // Re-checked when the person changed it, as an agent proposal is: what the
    // model proposed passed on the way in, and an edited name or trigger line
    // never has. The bundling check is not run again — the scripts are the
    // model's and weren't edited, and a procedure the person rewrote to name a
    // file is one they mean to add.
    let proposal = match response.edited {
        Some(edited) => {
            taurus_skills::proposal::validate_proposal(
                &edited,
                &*state.host.catalog().read().await,
            )
            .map_err(|e| format!("this skill can't be saved as written: {e}"))?;
            edited
        }
        None => original,
    };
    let root = match response.target.unwrap_or(SaveTarget::Project) {
        SaveTarget::Project => {
            taurus_host::config::workspace_skills_dir(&state.host.workspace().await)
        }
        SaveTarget::User => taurus_host::config::user_skills_dir(),
    };

    let dir = save(&proposal, &root).map_err(|e| format!("could not save skill: {e}"))?;
    info!(skill = %proposal.name, dir = %dir.display(), "skill approved");

    // Reloaded so the skill is usable in the session that just proposed it.
    // The local half only: a skill changes nothing an MCP server is running
    // with, so there is nothing to restart one for.
    state.host.reload_local().await;
    // And so the count on the rail moves with it, rather than on whatever the
    // user does next.
    emit_status(&state).await;
    Ok(Some(dir.display().to_string()))
}

/// Retunes one agent's iteration limit. Returns the file that now holds it,
/// which for a built-in is an override that did not exist a moment ago.
#[tauri::command]
pub async fn set_agent_iterations(
    state: State<'_, Arc<AppState>>,
    name: String,
    limit: u32,
) -> CmdResult<String> {
    state.host.set_agent_iterations(&name, limit).await
}

#[tauri::command]
pub async fn set_max_iterations(state: State<'_, Arc<AppState>>, limit: u32) -> CmdResult<()> {
    state.host.set_max_iterations(limit).await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn set_max_concurrent_agents(
    state: State<'_, Arc<AppState>>,
    count: u32,
) -> CmdResult<()> {
    state.host.set_max_concurrent_agents(count).await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn set_context_limit(state: State<'_, Arc<AppState>>, limit: u32) -> CmdResult<()> {
    state.host.set_context_limit(limit).await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn set_skill_synthesis(state: State<'_, Arc<AppState>>, enabled: bool) -> CmdResult<()> {
    state.host.set_skill_synthesis(enabled).await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn list_agent_proposals(
    state: State<'_, Arc<AppState>>,
) -> CmdResult<Vec<AgentProposal>> {
    Ok(state
        .pending_agent_proposals
        .iter()
        .map(|e| e.value().clone())
        .collect())
}

/// Snake case and exported, like [`ProposalResponse`] beside it. Every field
/// is one word, so the wire is what it was; the TypeScript is now checked
/// against it rather than written to match it by hand.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct AgentProposalResponse {
    pub id: String,
    pub approve: bool,
    /// Where to save it. Ignored when rejecting.
    #[serde(default)]
    pub target: Option<AgentSaveTarget>,
    /// The user's edits, if they changed anything in the review card.
    #[serde(default)]
    pub edited: Option<AgentProposal>,
}

#[tauri::command]
pub async fn respond_agent_proposal(
    state: State<'_, Arc<AppState>>,
    response: AgentProposalResponse,
) -> CmdResult<Option<String>> {
    let Some((_, original)) = state.pending_agent_proposals.remove(&response.id) else {
        return Err(format!("no pending agent proposal '{}'", response.id));
    };

    if !response.approve {
        info!(agent = %original.name, "agent proposal rejected");
        return Ok(None);
    }

    let proposal = response.edited.unwrap_or(original);

    // Re-validated because the card is editable. What the model proposed passed
    // on the way in; what the user is about to save may be something else
    // entirely, and a hand-edited name or tool list has never been checked.
    let root = validated_agent_root(
        &state,
        &proposal,
        response.target.unwrap_or(AgentSaveTarget::Project),
    )
    .await?;

    let path =
        save_agent_file(&proposal, &root).map_err(|e| format!("could not save agent: {e}"))?;
    info!(agent = %proposal.name, path = %path.display(), "agent approved");

    // Narrower than `reload`, which would restart every MCP server to pick up
    // one markdown file.
    state.host.rescan_agents().await;
    emit_status(&state).await;
    Ok(Some(path.display().to_string()))
}

/// Turns the terminal's shell integration on or off. See
/// [`taurus_host::Host::set_shell_integration`].
#[tauri::command]
pub async fn set_context_strategy(
    state: State<'_, Arc<AppState>>,
    strategy: taurus_core::ContextStrategy,
) -> CmdResult<()> {
    state.host.set_context_strategy(strategy).await;
    emit_status(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn set_shell_integration(
    state: State<'_, Arc<AppState>>,
    enabled: bool,
) -> CmdResult<()> {
    state.host.set_shell_integration(enabled).await;
    Ok(())
}

#[tauri::command]
pub async fn set_agent_synthesis(state: State<'_, Arc<AppState>>, enabled: bool) -> CmdResult<()> {
    state.host.set_agent_synthesis(enabled).await;
    emit_status(&state).await;
    Ok(())
}

/// Checks a draft agent against the tools this session has and the agents it
/// already knows, and says which folder it would be saved in.
///
/// Both ways an agent is saved run this: the editor's Save, whose draft has
/// never been checked, and an approved proposal, re-checked because the card
/// the user approved it from is editable. Written twice, the two had begun to
/// differ only in the order of their lines.
async fn validated_agent_root(
    state: &AppState,
    draft: &AgentProposal,
    target: AgentSaveTarget,
) -> Result<std::path::PathBuf, String> {
    let available: Vec<String> = state
        .host
        .registry()
        .read()
        .await
        .names()
        .map(str::to_string)
        .collect();
    {
        let catalog = state.host.agent_catalog().read().await;
        validate_agent(draft, &catalog, &available)
            .map_err(|e| format!("this agent cannot be saved as written: {e}"))?;
    }
    Ok(match target {
        AgentSaveTarget::Project => {
            taurus_host::config::workspace_agents_dir(&state.host.workspace().await)
        }
        AgentSaveTarget::User => taurus_host::config::user_agents_dir(),
    })
}

/* ------------------------------------------------------------- authoring */

/// Where a skill draft is filed in [`AppState::stoppable`].
pub(super) const SKILL_DRAFT: &str = "skill-draft";

/// Ends a draft [`generate_skill`] is still waiting on.
#[tauri::command]
pub fn stop_skill_draft(state: State<'_, Arc<AppState>>) {
    state.stoppable.stop(SKILL_DRAFT);
}

/// What the model is told to produce for the skill editor's Generate button.
/// JSON for the reason [`draft_system`] gives: every field lands in a box the
/// person can correct, and none of it is YAML that is nearly right.
pub(super) fn skill_draft_system() -> String {
    format!(
        "\
You draft a skill for a coding agent: a written procedure it loads when the \
situation calls for it. Reply with JSON only — no prose, no markdown fence.

Fields:
- name: kebab-case, specific, e.g. \"cut-release\"
- description: one sentence on what the skill does.
- when_to_use: the situation in which the agent should open this skill, under \
{TRIGGER} characters. It is the only text the agent sees when deciding, so \
describe the trigger, not the capability.
- body: the procedure, as markdown. Numbered steps the agent can follow \
without asking anything: the commands to run, what to check, what to report. \
Do not name script files; this skill has none yet.",
        TRIGGER = taurus_skills::skill::TRIGGER_MAX,
    )
}

#[derive(Deserialize)]
pub(super) struct DraftedSkill {
    name: String,
    description: String,
    when_to_use: String,
    body: String,
}

/// Drafts a skill from a description, for the editor to fill in. A starting
/// point, like [`generate_agent`]: it lands in the editor's fields and the
/// person saves it.
#[tauri::command]
pub async fn generate_skill(
    state: State<'_, Arc<AppState>>,
    description: String,
    provider_id: String,
    model: String,
) -> CmdResult<SkillProposal> {
    if description.trim().is_empty() {
        return Err("describe what the skill should do first".into());
    }
    let prompt = format!("Draft a skill for: {}", description.trim());
    let text = one_shot(
        &state,
        SKILL_DRAFT,
        &provider_id,
        &model,
        skill_draft_system(),
        prompt,
    )
    .await?;
    let json = extract_json(&text).ok_or_else(|| {
        format!(
            "{model} did not answer with JSON. It said: {}",
            brief(&text)
        )
    })?;
    let drafted: DraftedSkill = serde_json::from_str(json)
        .map_err(|e| format!("{model} answered with JSON that does not fit a skill: {e}"))?;
    info!(skill = %drafted.name, "skill drafted");
    Ok(SkillProposal::new(
        drafted.name.trim(),
        drafted.description.trim(),
        drafted.when_to_use.trim(),
        drafted.body.trim(),
    ))
}

/// Saves a skill written in the editor. Answers with its folder. See
/// [`taurus_host::Host::create_skill`].
#[tauri::command]
pub async fn create_skill(
    state: State<'_, Arc<AppState>>,
    draft: SkillProposal,
    target: SaveTarget,
    plugin: Option<String>,
) -> CmdResult<String> {
    let dir = state
        .host
        .create_skill(&draft, target, plugin.as_deref())
        .await?;
    emit_status(&state).await;
    Ok(dir)
}

/// Deletes a skill Taurus wrote. Answers with the folder that went.
#[tauri::command]
pub async fn delete_skill(state: State<'_, Arc<AppState>>, name: String) -> CmdResult<String> {
    let dir = state.host.delete_skill(&name).await?;
    emit_status(&state).await;
    Ok(dir)
}

/// Copies a skill into your own folder of the same tier, to change there.
/// Answers with the copy's folder.
#[tauri::command]
pub async fn copy_skill(state: State<'_, Arc<AppState>>, name: String) -> CmdResult<String> {
    let dir = state.host.copy_skill(&name).await?;
    emit_status(&state).await;
    Ok(dir)
}

/// The file to open in the canvas to change an agent, writing a copy of your
/// own first when it can't be changed where it is.
#[tauri::command]
pub async fn agent_file(state: State<'_, Arc<AppState>>, name: String) -> CmdResult<String> {
    let path = state.host.agent_file(&name).await?;
    emit_status(&state).await;
    Ok(path)
}

/// Deletes an agent file Taurus owns. Answers with the file that went.
#[tauri::command]
pub async fn delete_agent(state: State<'_, Arc<AppState>>, name: String) -> CmdResult<String> {
    let path = state.host.delete_agent(&name).await?;
    emit_status(&state).await;
    Ok(path)
}

/// The `TAURUS.md` for a tier, created empty if it isn't there, for the canvas
/// to open.
#[tauri::command]
pub async fn start_brief(
    state: State<'_, Arc<AppState>>,
    tier: taurus_host::InstructionsTier,
) -> CmdResult<String> {
    state.host.start_brief(tier).await
}
