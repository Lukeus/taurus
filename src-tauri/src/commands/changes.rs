//! What a turn changed: checkpoints, rewind, review, commit, and the reports about past turns.

use super::*;

#[tauri::command]
pub async fn list_checkpoints(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> CmdResult<Vec<Checkpoint>> {
    // Off the runtime: `turns` reads the whole checkpoint log. It parses only
    // the names out of it, passing over each pre-image without copying it, but
    // a long session's log is still tens of megabytes to read through.
    let store = state
        .host
        .checkpoints_for(&session_workspace(&state, &session_id).await);
    off_runtime(move || store.turns(&session_id)).await
}

/// Restores the workspace to just before `turn`.
///
/// With `dry_run`, reports what that would do and writes nothing — which is how
/// the UI shows the plan before asking.
#[tauri::command]
pub async fn rewind_to(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    turn: u32,
    dry_run: bool,
) -> CmdResult<Rewind> {
    // A turn holds this lock for its whole run. Rewinding underneath one would
    // race the tool calls still writing, and the disabled button in the UI is
    // not something the backend should have to trust.
    if let Ok(entry) = state.session(&session_id) {
        let _ = entry.idle("stop it before rewinding")?;
    }

    // The conversation's own folder, which is where its pre-images came from.
    // Resolved against the open one instead, a rewind reached for a log that
    // is not there and reported nothing to undo — and had it found one, it
    // would have restored a different project's files.
    let workspace = session_workspace(&state, &session_id).await;
    // Off the runtime: a rewind reads the whole log, then every file it names,
    // and writes them back. On a long session that is long enough to stall the
    // stream and the permission prompt that share the runtime with it.
    let store = state.host.checkpoints_for(&workspace);
    let rewind = {
        let session_id = session_id.clone();
        off_runtime(move || store.rewind(&session_id, &workspace, turn, dry_run)).await?
    };

    // The checklist is working state, and rewinding is undoing the work it
    // tracked. Kept, it would be the one thing in the session that still
    // believes in a turn nothing else remembers — the model reading a plan for
    // files that have been put back. A dry run changes nothing, so it clears
    // nothing.
    if !dry_run {
        state.host.forget_plan(&session_id).await;
        // The header counts files this conversation changed, and a rewind is
        // the only thing that makes that number go down. The drawer re-reads
        // its own list; this is for the count behind it.
        emit_changed(&state, &session_id).await;
    }
    Ok(rewind)
}

/// What one turn changed, file by file, as a diff.
///
/// Read on demand rather than sent with the listing: a session of thirty turns
/// would otherwise ship every diff it ever made to draw a drawer, and the
/// drawer shows one turn expanded at a time.
#[tauri::command]
pub async fn turn_changes(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    turn: u32,
) -> CmdResult<Vec<TurnChange>> {
    let workspace = session_workspace(&state, &session_id).await;
    // Off the runtime, for the reason `list_checkpoints` is: the whole log is
    // read and each file diffed, and the drawer is opened mid-turn.
    let store = state.host.checkpoints_for(&workspace);
    off_runtime(move || store.changes(&session_id, &workspace, turn)).await
}

/// Reads one turn back to an agent that did not write it.
///
/// Runs on the conversation's own provider and model, resolved the same way a
/// turn is — a review that quietly ran somewhere else would be a bill nobody
/// authorised — but it is not a turn: nothing is recorded, nothing reaches the
/// transcript, and the conversation's cancellation token is left alone so a
/// review and the turn it is about cannot stop each other.
///
/// It takes minutes on a local model, which is why the drawer says so before
/// the button rather than after it. See [`taurus_host::review`].
#[tauri::command]
pub async fn review_turn(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    turn: u32,
) -> CmdResult<taurus_host::review::ReviewReport> {
    let entry = state.session(&session_id)?;
    check_workspace(&entry.workspace, &state.host.workspace().await)?;

    let provider_id = entry.provider_id.lock().await.clone();
    let provider = state.host.provider(&provider_id).await?;
    let model = session_model(&entry).await;

    // A token of its own rather than the conversation's. Stopping a turn must
    // not kill a review of an earlier one, and closing a review must not stop
    // the turn running beside it. Filed under the turn it reads, so the drawer
    // can end it: see `stop_review`.
    let running = state.stoppable.start(review_key(&session_id, turn));

    state
        .host
        .review_turn(provider, &model, &session_id, turn, running.cancel.clone())
        .await
}

/// Ends a review [`review_turn`] is still waiting on.
///
/// Safe to call when none is running: the drawer calls it on the way out
/// without knowing whether its review already came back.
#[tauri::command]
pub fn stop_review(state: State<'_, Arc<AppState>>, session_id: String, turn: u32) {
    state.stoppable.stop(&review_key(&session_id, turn));
}

/// Where a review of one turn is filed in [`AppState::stoppable`].
pub(super) fn review_key(session_id: &str, turn: u32) -> String {
    format!("review/{session_id}/{turn}")
}

/// What the whole conversation changed, file by file, as one diff each.
///
/// The question asked before a commit, which the per-turn view cannot answer:
/// a file edited in three turns has three diffs there and one here. See
/// [`taurus_tools::checkpoint::CheckpointStore::changes_all`] for why that is
/// not the same list read twice.
#[tauri::command]
pub async fn conversation_changes(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> CmdResult<Vec<TurnChange>> {
    let workspace = session_workspace(&state, &session_id).await;
    // Off the runtime, the same as a single turn's diffs — this is all of them.
    let store = state.host.checkpoints_for(&workspace);
    off_runtime(move || store.changes_all(&session_id, &workspace)).await
}

/// Where the workspace stands with git, for the branch label and the commit
/// button.
#[tauri::command]
pub async fn repo_status(state: State<'_, Arc<AppState>>) -> CmdResult<RepoStatus> {
    Ok(state.host.repo_status().await)
}

/// Conversations mentioning `query`, newest first.
///
/// `everywhere` searches every workspace rather than the open one — the
/// question "where did I do that", asked when you no longer remember which
/// project it was.
///
/// Off the runtime: this reads every transcript in the workspace, and it is
/// called while somebody is typing. What keeps that affordable is in
/// [`taurus_host::search`] — a conversation that does not mention the query is
/// one file read and nothing parsed.
#[tauri::command]
pub async fn search_sessions(
    state: State<'_, Arc<AppState>>,
    query: String,
    everywhere: bool,
) -> CmdResult<SearchResults> {
    let workspace = if everywhere {
        None
    } else {
        Some(state.host.workspace().await)
    };
    off_runtime(move || Ok(search::search(workspace.as_deref(), &query))).await
}

/// Where the context window actually went.
///
/// `session_id` names one conversation; `None` accounts for every saved
/// conversation in the open workspace. Both answers come from
/// [`taurus_host::usage`] rather than from anything here — the CLI prints the
/// same report, and a tool's cost that differed between the two would be a
/// number nobody could act on.
///
/// The fixed half — the system prompt and every advertised tool schema — is
/// read off the *live* host rather than out of the transcript, so what it
/// reports is what the next request will cost rather than what an earlier one
/// did. That is also why it is worth asking for in a workspace with no history
/// at all.
#[tauri::command]
pub async fn usage_report(
    state: State<'_, Arc<AppState>>,
    session_id: Option<String>,
) -> CmdResult<UsageReport> {
    let fixed = usage::Fixed::new(
        &state.host.system_prompt().await,
        state.host.tool_definitions().await,
    );

    // A conversation that is open answers from memory, which is what the
    // window is working from — when it can. This panel is most often opened
    // mid-turn, to find out what just filled the window, and a turn holds the
    // session for its whole run: waiting for it would keep the panel spinning
    // until the turn is over. Mid-turn it reads the transcript instead, which
    // the turn writes at the end of every tool round, so it is behind by at
    // most the round in flight. `resume_session` falls back the same way.
    if let Some(id) = &session_id {
        if let Ok(entry) = state.session(id) {
            if let Some(report) = open_usage(&entry, &fixed) {
                return Ok(report);
            }
        }
    }

    let workspace = match &session_id {
        Some(id) => session_workspace(&state, id).await,
        None => state.host.workspace().await,
    };
    // Reading forty transcripts off disk is not something to do on the
    // runtime, and the workspace-wide view is exactly when there are forty.
    off_runtime(move || {
        usage::report(
            &workspace,
            session_id.as_deref(),
            session_id.is_none(),
            &fixed,
        )
    })
    .await
}

/// An open conversation's account, from memory — or `None` while a turn is
/// running in it, for the caller to read off disk instead. See `usage_report`.
pub(super) fn open_usage(entry: &SessionEntry, fixed: &usage::Fixed) -> Option<UsageReport> {
    let session = entry.session.try_lock().ok()?;
    Some(usage::of_session(&session, fixed))
}

/// Where a turn's time actually went.
///
/// `session_id` names one conversation; `None` reports on everything this
/// window has run, which is the "is it always like this" view — and, unlike
/// the usage account, it can answer about conversations that have since been
/// closed, because the source is a ring in this process rather than a file on
/// disk.
///
/// Nothing is read off disk at all, so this stays on the runtime: the whole of
/// it is arithmetic over a bounded buffer that is already in memory. That is
/// also the honest limit of this panel — it describes what *this run of the
/// app* has done, and it forgets on quit. Durable history is what an OTLP
/// endpoint is for, and both can be on at once.
#[tauri::command]
pub async fn trace_report(
    state: State<'_, Arc<AppState>>,
    session_id: Option<String>,
) -> CmdResult<TraceReport> {
    Ok(traces::report(&state.traces, session_id.as_deref()))
}

/// Forgets every span recorded so far.
///
/// For somebody about to measure one specific thing, who wants the next
/// reading to be of it rather than of it plus the morning. Nothing else is
/// affected: an exporter, if one is configured, has already sent what it sent.
#[tauri::command]
pub async fn clear_traces(state: State<'_, Arc<AppState>>) -> CmdResult<()> {
    state.traces.clear();
    Ok(())
}

/// Commits exactly the files one turn changed.
///
/// The turn is named rather than the paths, so the frontend cannot ask for a
/// commit of files that turn did not touch — the checkpoint log is the only
/// thing that decides what goes in.
#[tauri::command]
pub async fn commit_turn(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    turn: u32,
    message: String,
) -> CmdResult<Commit> {
    // The same rule `rewind_to` applies: a turn holds this lock for its whole
    // run, and committing underneath one would capture a tree that is still
    // being written.
    if let Ok(entry) = state.session(&session_id) {
        let _ = entry.idle("wait for it to finish before committing")?;
    }

    // The conversation's own folder, which is the repository its turns changed
    // files in — not whichever one is open now.
    let workspace = session_workspace(&state, &session_id).await;
    let checkpoints = state.host.checkpoints_for(&workspace);

    // Re-read rather than trusting a path list from the frontend, so what is
    // committed is what was recorded. Off the runtime, the same read
    // `list_checkpoints` makes: the whole log, for one turn's file names.
    let listing = state.host.checkpoints_for(&workspace);
    let id = session_id.clone();
    let files = off_runtime(move || listing.turns(&id))
        .await?
        .into_iter()
        .find(|checkpoint| checkpoint.turn == turn)
        .map(|checkpoint| checkpoint.files)
        .ok_or_else(|| format!("turn {turn} is not in this conversation's checkpoint log"))?;

    let repo = Repo::discover(&workspace)
        .await?
        .ok_or("This workspace is not a git repository, so there is nothing to commit to.")?;

    let commit = repo.commit(&files, &message).await?;

    // After the commit, because until git has made one there is no sha to write
    // down. This is what lets the drawer say which turns are already in `HEAD`
    // — so a commit of turn 5 can warn that turn 4 is not in one, and a rewind
    // past turn 3 can say the commit it is about to orphan.
    checkpoints.record_commit(&session_id, &workspace, turn, &commit.sha);

    info!(
        session = %session_id,
        turn,
        sha = %commit.sha,
        files = commit.files.len(),
        skipped = commit.skipped.len(),
        "committed a turn"
    );
    Ok(commit)
}

/* ----------------------------------------------------------- background */

/// One look at the background commands, for the dock that draws them.
///
/// One round trip rather than two, because the pane asks on a timer and the
/// two halves are read together every time: which commands there are, and what
/// the one on screen has said since the last look.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct Background {
    pub jobs: Vec<BackgroundJob>,
    /// Absent when the pane is watching nothing, and when the command it is
    /// watching is no longer there — a workspace change forgets them, and a
    /// pane mid-poll finds out from this rather than from an error.
    #[ts(optional)]
    pub output: Option<JobOutput>,
}
