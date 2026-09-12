//! Conversations: starting, resuming, sending, and the questions a turn asks.

use super::*;

#[tauri::command]
pub async fn list_models(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
) -> CmdResult<Vec<ModelInfo>> {
    let provider = state.host.provider(&provider_id).await?;
    provider.models().await.map_err(|e| e.to_string())
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct CreatedSession {
    pub id: String,
    pub model: String,
    pub provider_id: String,
    /// False when the model has no native tool support and the prompted
    /// fallback will be used. Shown in the UI because it changes reliability.
    pub native_tools: bool,
    /// Whether this model reads images. Decides whether the composer offers to
    /// attach one at all — an attach button on a model that cannot see is an
    /// invitation to a refusal.
    pub vision: bool,
    pub context_length: u32,
}

#[tauri::command]
pub async fn create_session(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
    model: String,
) -> CmdResult<CreatedSession> {
    let provider = state.host.provider(&provider_id).await?;
    let capabilities = provider
        .capabilities(&model)
        .await
        .map_err(|e| e.to_string())?;

    let session = Session::new(&model);
    // Stamped at creation rather than looked up later: which branch the work
    // was done on is only knowable while it is happening. The same is true of
    // the workspace, which is why the entry carries it from here on.
    let workspace = state.host.workspace().await;
    let branch = state.host.branch().await;
    let log = SessionLog::create(&session, &workspace, branch);
    let id = session.id.clone();
    state.sessions.insert(
        id.clone(),
        Arc::new(SessionEntry {
            session: Arc::new(Mutex::new(session)),
            provider_id: Mutex::new(provider_id.clone()),
            model: Mutex::new(model.clone()),
            workspace,
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
            log: Arc::new(Mutex::new(log)),
            // A conversation starts where it starts; a switch is what puts
            // anything in here.
            switches: Mutex::new(Vec::new()),
        }),
    );

    state.host.remember_session(&provider_id, &model).await;

    info!(session = %id, %model, native_tools = capabilities.native_tools, "session created");
    Ok(CreatedSession {
        id,
        model,
        provider_id,
        native_tools: capabilities.native_tools,
        vision: capabilities.vision,
        context_length: capabilities.context_length,
    })
}

/// Saved conversations, newest first — this workspace's, or every one.
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, Arc<AppState>>,
    all: bool,
) -> CmdResult<Vec<SessionMeta>> {
    // Every transcript in the workspace is opened and partly parsed to build
    // this, and with `all` every transcript on the machine is — so it is one of
    // the two reads that most needs to be off the runtime.
    let workspace = state.host.workspace().await;
    off_runtime(move || Ok(sessions::list(if all { None } else { Some(&workspace) }))).await
}

/// What a resumed conversation needs to be redrawn and continued.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct ResumedSession {
    pub id: String,
    pub model: String,
    pub provider_id: String,
    pub native_tools: bool,
    /// Whether this model reads images. See [`CreatedSession::vision`].
    pub vision: bool,
    pub context_length: u32,
    /// The whole transcript, for the frontend to rebuild the view from.
    pub messages: Vec<Message>,
    /// Where this conversation changed model, each positioned by how much of
    /// the transcript came before it — so a reopened conversation shows the
    /// change where it happened rather than only what it ended on.
    pub switches: Vec<Switch>,
}

/// The conversation one delegate had, for reading.
///
/// Deliberately not a resume. A delegate's transcript is a record of work that
/// happened inside a turn, not a conversation to be carried on: it has no
/// provider bound to it, no workspace of its own, and continuing it would mean
/// a second live session nobody asked for. The frontend gets the messages and
/// draws them read-only.
///
/// Scoped to the parent, because that is what a delegate's id is unique
/// *within*. Both ids are validated against the sessions tree before either
/// touches the filesystem — see `taurus_host::sessions`.
#[tauri::command]
pub async fn read_subagent_transcript(
    session_id: String,
    subagent_id: String,
) -> CmdResult<Vec<Message>> {
    off_runtime(move || {
        sessions::load_subagent(&session_id, &subagent_id).map(|loaded| loaded.session.messages)
    })
    .await
}

/// Reopens a saved conversation as a live session.
///
/// Already-open sessions are returned as they stand rather than reloaded: the
/// in-memory one is the newer of the two mid-turn, and replacing it would drop
/// whatever the running turn has produced.
#[tauri::command]
pub async fn resume_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    provider_id: Option<String>,
) -> CmdResult<ResumedSession> {
    // `loaded` is carried alongside so the log below can be opened on the
    // transcript this was actually read from — see `SessionLog::resume`.
    let (session, provider_id, switches, loaded) = match state.sessions.get(&session_id) {
        Some(open) => {
            let entry = open.clone();
            let provider_id = entry.provider_id.lock().await.clone();
            let switches = entry.switches.lock().await.clone();
            // Never waited for. A turn holds this lock for its whole run, which
            // is minutes for a long one, and awaiting it here hung the window
            // on the click that reopens the conversation that is *already*
            // streaming — the one case where the frontend does not close the
            // previous session first, so the one case that could reach it.
            //
            // `delete` and `rewind` meet the same lock and refuse; this does
            // not, because reopening a conversation has to work. What it falls
            // back to is what a cold open would have read: the transcript on
            // disk, complete to the end of the last round. The turn's own
            // stream carries on appending from there.
            let live = entry.session.try_lock().ok().map(|s| s.clone());
            match live {
                Some(session) => (session, provider_id, switches, None),
                None => {
                    let id = session_id.clone();
                    let loaded = off_runtime(move || sessions::load(&id)).await?;
                    // Deliberately not `Some(loaded)`: that is what opens a log
                    // and installs a session entry, and this conversation
                    // already has both — with a turn running through them.
                    (loaded.session, provider_id, switches, None)
                }
            }
        }
        None => {
            // The whole `.jsonl` — file contents, shell output, MCP results
            // and all — parsed a line at a time. Several megabytes is ordinary
            // for a long coding conversation, and this is the click that used
            // to stutter an unrelated live stream.
            let id = session_id.clone();
            let loaded = off_runtime(move || sessions::load(&id)).await?;
            // Whichever provider the caller is on; failing that the one this
            // conversation was last worked on, which is known only for one that
            // has moved at least once; failing that whatever the host resolves.
            // A header records the model but deliberately not the backend that
            // served it, and that backend may not even be configured now.
            let (resolved, _) = state
                .host
                .resolve_model(
                    provider_id.as_deref().or(loaded.provider.as_deref()),
                    Some(&loaded.session.model),
                )
                .await?;
            let switches = loaded.switches.clone();
            (loaded.session.clone(), resolved, switches, Some(loaded))
        }
    };

    let provider = state.host.provider(&provider_id).await?;
    let capabilities = provider
        .capabilities(&session.model)
        .await
        .map_err(|e| e.to_string())?;

    let resumed = ResumedSession {
        id: session.id.clone(),
        model: session.model.clone(),
        provider_id: provider_id.clone(),
        native_tools: capabilities.native_tools,
        vision: capabilities.vision,
        context_length: capabilities.context_length,
        messages: session.messages.clone(),
        switches: switches.clone(),
    };

    // Only if it is still absent. The awaits above are where a second resume of
    // the same conversation would arrive, and the entry it had already made is
    // the newer of the two — an insert here would drop the log it is appending
    // through and its in-flight turn's cancellation token with it.
    if let Some(loaded) = loaded {
        let log = SessionLog::resume(&loaded);
        if let Entry::Vacant(slot) = state.sessions.entry(session_id.clone()) {
            slot.insert(Arc::new(SessionEntry {
                session: Arc::new(Mutex::new(session)),
                provider_id: Mutex::new(provider_id),
                model: Mutex::new(resumed.model.clone()),
                // The conversation's own folder, out of its header — not the
                // one open now. They are the same in the ordinary case and
                // must not be assumed to be.
                workspace: loaded.workspace,
                cancel: Arc::new(Mutex::new(CancellationToken::new())),
                log: Arc::new(Mutex::new(log)),
                switches: Mutex::new(switches),
            }));
            info!(session = %session_id, "session resumed");
        }
    }

    Ok(resumed)
}

/// Runs one turn, streaming events to `on_event`.
///
/// A `Channel` rather than a global event: delivery is ordered and scoped to
/// this call, so two sessions streaming at once cannot interleave in the UI.
#[tauri::command]
pub async fn send_message(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    text: String,
    images: Option<Vec<Attachment>>,
    // What the Data pane was showing, when the message was sent from it. It
    // reaches the model and not the transcript — see `taurus_host::onscreen`,
    // which explains the split and what it costs.
    on_screen: Option<OnScreen>,
    on_event: Channel<UiEvent>,
) -> CmdResult<()> {
    let entry = state.session(&session_id)?;

    check_workspace(&entry.workspace, &state.host.workspace().await)?;

    // Read once, here, so a turn is sent to the backend this conversation was
    // on when it began even if somebody moves it while the answer streams.
    let provider_id = entry.provider_id.lock().await.clone();
    let provider = state.host.provider(&provider_id).await?;

    // `/name args` becomes the skill's procedure — or the instruction to
    // delegate to that sub-agent — before the model sees it. The user's own
    // line stays what the transcript shows and what names the turn: an
    // expansion is how the request is carried out, not what was asked.
    let prompt = match state.host.expand_command(&text).await {
        Some(Ok(invocation)) => {
            info!(
                session = %session_id,
                command = %invocation.name,
                kind = ?invocation.kind,
                "ran a command",
            );
            invocation.prompt
        }
        // Returned rather than sent. A mistyped command is a message to the
        // user, and passing it to the model would answer a question about a
        // skill instead of running one.
        Some(Err(e)) => return Err(e.to_string()),
        None => text.clone(),
    };
    // After the command expansion rather than before it: a `/skill` invocation
    // is still being sent from a pane, and the procedure it expands to wants
    // the same subject the user had on screen.
    let prompt = taurus_host::onscreen::with_context(&prompt, on_screen.as_ref());

    // Checked before anything is started, and against this model rather than
    // this provider: on Ollama one model on a server reads images and the next
    // one does not. Refusing here costs the user a retry with the same text;
    // letting it through costs a round trip and comes back as a wire error
    // naming a field in the request body.
    //
    // Out of the session itself, and waited for rather than read off the
    // entry: a message sent while a turn is running waits here for that turn
    // to end, rather than building its agent underneath it.
    let model = entry.session.lock().await.model.clone();
    let images = images.unwrap_or_default();
    let blocks = if images.is_empty() {
        Vec::new()
    } else {
        let capabilities = provider
            .capabilities(&model)
            .await
            .map_err(|e| e.to_string())?;
        taurus_host::attach::to_blocks(&images, &capabilities)?
    };

    // A fresh token per turn: reusing a canceled one would abort the next turn
    // before it started.
    let cancel = CancellationToken::new();
    *entry.cancel.lock().await = cancel.clone();

    let agent = state
        .host
        .build_agent(
            provider,
            &model,
            cancel,
            TurnRef {
                session_id: &session_id,
                prompt: &text,
            },
        )
        .await
        // Persistence moves into the loop, which records once per tool round
        // trip and once when the turn ends however it ends. Two things follow.
        // The question reaches disk before the model is asked it, so a turn
        // killed half way leaves what was asked rather than nothing. And the
        // conversation becomes listable while it is being answered — with its
        // title — where before it appeared in the rail only once the turn was
        // over, which for a long turn is minutes of the app disagreeing with
        // itself about which conversations exist.
        .with_recorder(Arc::new(crate::bridge::UiSessionLog::new(
            state.app.clone(),
            entry.log.clone(),
            session_id.clone(),
        )));

    // Bridge the loop's mpsc channel to the IPC channel.
    let (tx, mut rx) = mpsc::channel::<UiEvent>(256);
    let forwarder = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if on_event.send(event).is_err() {
                break;
            }
        }
    });

    // Images first, then the text. A model answers "what is wrong with this?"
    // better having already seen the thing, and every adapter here preserves
    // block order.
    let message = if blocks.is_empty() {
        Message::user(prompt)
    } else {
        let mut content = blocks;
        content.push(taurus_provider::ContentBlock::text(prompt));
        Message::new(taurus_provider::Role::User, content)
    };

    let mut session = entry.session.lock().await;
    // The transcript is written from inside this call, by the recorder attached
    // above — once per round and once at the end, whatever the outcome. An
    // interrupted turn still produced the messages that led there, and they are
    // already on disk in the order they happened.
    let outcome = agent.run_turn(&mut session, message, tx).await;
    drop(session);
    let _ = forwarder.await;

    // The conversation's listing entry has moved: its timestamp, and its title
    // if this was its first turn. The status has too — a turn can leave a note
    // behind, and can be the thing that moved the branch.
    //
    // Pushed from a task of its own rather than awaited. The window learns the
    // turn is over from this call returning, and awaiting a transcript read and
    // a whole status first kept the Stop button lit after the answer had ended.
    {
        let state = state.inner().clone();
        let session_id = session_id.clone();
        tauri::async_runtime::spawn(async move {
            emit_session(&state, &session_id).await;
            emit_status(&state).await;
        });
    }

    match outcome {
        Ok(outcome) => {
            info!(
                session = %session_id,
                iterations = outcome.iterations,
                "turn finished"
            );
            Ok(())
        }
        Err(e) => {
            error!(session = %session_id, error = %e, "turn failed");
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn cancel_session(state: State<'_, Arc<AppState>>, session_id: String) -> CmdResult<()> {
    state.session(&session_id)?.cancel.lock().await.cancel();
    info!(session = %session_id, "cancel requested");
    Ok(())
}

#[tauri::command]
pub async fn close_session(state: State<'_, Arc<AppState>>, session_id: String) -> CmdResult<()> {
    if let Some((_, entry)) = state.sessions.remove(&session_id) {
        entry.cancel.lock().await.cancel();
    }
    // The board goes with the conversation it belongs to. `delete_session` and
    // `rewind_to` already did this; closing did not, and closing is the one
    // that happens on every switch between conversations — so the map grew by
    // one for each and never shrank. Nothing can reach a board whose session
    // entry is gone, which is what made it a leak rather than a cache.
    state.host.forget_plan(&session_id).await;
    Ok(())
}

/// Deletes a saved conversation: the transcript, and the checkpoints that made
/// its turns undoable.
///
/// Both halves go, because half of the record is worse than none — a checkpoint
/// log outliving its conversation is a copy of the user's files under an id
/// nothing can reach. Nothing in the workspace itself is touched: this forgets
/// what was said and how to undo it, not the work.
#[tauri::command]
pub async fn delete_session(state: State<'_, Arc<AppState>>, session_id: String) -> CmdResult<()> {
    // The same rule `rewind_to` applies, for the same reason: a turn holds this
    // lock for its whole run, and deleting underneath one would leave it
    // appending to a file that is no longer anywhere.
    if let Ok(entry) = state.session(&session_id) {
        let _ = entry.idle("stop it before deleting")?;
    }

    // Dropped from memory before the file goes, not after. An open session's log
    // recreates the transcript on its next write, so the other order can leave a
    // deleted conversation back on disk.
    if let Some((_, entry)) = state.sessions.remove(&session_id) {
        entry.cancel.lock().await.cancel();
    }

    // Both off the runtime: a transcript can be megabytes, and the checkpoint
    // log beside it more.
    let checkpoints = state.host.checkpoints().await;
    let id = session_id.clone();
    off_runtime(move || {
        sessions::delete(&id)?;
        checkpoints.forget(&id)
    })
    .await?;
    state.host.forget_plan(&session_id).await;
    info!(session = %session_id, "session deleted");
    Ok(())
}

/// Moves a conversation to another model, or another backend, keeping
/// everything said in it.
///
/// The alternative — which is what both pickers used to do — is a new
/// conversation, and that is a poor trade for a question you wanted a second
/// opinion on. Nothing about the history is provider-shaped: it is stored as
/// blocks, and each adapter renders those into its own wire format on the way
/// out, drops the reasoning it cannot replay, and rewrites tool calls as text
/// for a model with no native tool support. So carrying a conversation across
/// is a matter of saying so, not of translating it.
///
/// What does change is the model's capabilities, which is why this answers with
/// them. A smaller context window compacts on the next turn, because the budget
/// is recomputed per turn from whatever model the session is on. A model that
/// cannot read images is sent the conversation with its pictures replaced by a
/// line saying one was there — see `taurus_core`'s `without_images`; the images
/// stay in the session and come back if the conversation moves to a model that
/// can see.
///
/// Reuses [`CreatedSession`] rather than declaring a near-copy: it is not the
/// creating that the shape describes, it is what the frontend has to know about
/// the live conversation, and after this call all of that has moved.
#[tauri::command]
pub async fn switch_model(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    provider_id: String,
    model: String,
) -> CmdResult<CreatedSession> {
    let entry = state.session(&session_id)?;

    // Asked before the lock is taken, and it is what makes this fail cleanly: a
    // model the backend will not serve leaves the conversation where it was
    // rather than moved to something that cannot answer it.
    let provider = state.host.provider(&provider_id).await?;
    let capabilities = provider
        .capabilities(&model)
        .await
        .map_err(|e| e.to_string())?;

    let after = {
        // The same rule a rewind and a delete follow, and for a sharper reason
        // than either: a turn reads the model out of the session on every
        // attempt, so moving it underneath one would send half an answer to one
        // backend and half to another.
        let mut session = entry.idle("stop it before changing model")?;
        session.model = model.clone();
        *entry.model.lock().await = model.clone();
        // Counted now, while no turn can be running. Asked for again below, the
        // lock would wait out a turn started in between.
        session.messages.len()
    };
    *entry.provider_id.lock().await = provider_id.clone();

    // Written down, so reopening the conversation continues it here rather than
    // on the model in its header. Nothing is appended for a conversation with
    // no transcript yet — the first turn writes a header naming this model
    // instead. See `SessionLog::record_model`.
    if entry.log.lock().await.record_model(&provider_id, &model) {
        entry.switches.lock().await.push(Switch {
            after,
            provider: provider_id.clone(),
            model: model.clone(),
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default(),
        });
    }

    state.host.remember_session(&provider_id, &model).await;
    info!(session = %session_id, %provider_id, %model, "conversation moved to another model");
    emit_status(&state).await;

    Ok(CreatedSession {
        id: session_id,
        model,
        provider_id,
        native_tools: capabilities.native_tools,
        vision: capabilities.vision,
        context_length: capabilities.context_length,
    })
}

/// Gives a conversation a title of its own, or takes one away.
///
/// An empty title is a clear rather than an error: the box the user typed in
/// starts out holding the derived title, and emptying it is how you say "go
/// back to that" — see [`sessions::rename`].
///
/// Allowed mid-turn, unlike deleting. A rename touches the transcript's header
/// and nothing a running turn is appending to, and stopping the turn to retitle
/// the conversation it is running in would be a strange thing to have to do.
#[tauri::command]
pub async fn rename_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    title: String,
) -> CmdResult<SessionMeta> {
    // The write is serialized against the log for this conversation when there
    // is one, so a rewrite cannot land between a turn's append and the next.
    // A conversation that is only on disk has nothing to serialize against.
    //
    // Off the runtime either way: a rename rewrites the whole transcript and
    // syncs it, and it is allowed mid-turn — so it must hold neither a worker
    // nor the lock the turn's next record waits on any longer than the write.
    let log = state
        .session(&session_id)
        .ok()
        .map(|entry| entry.log.clone());
    let held = match log {
        Some(log) => Some(log.lock_owned().await),
        None => None,
    };
    let (id, name) = (session_id.clone(), title.clone());
    let meta = off_runtime(move || {
        let _held = held;
        sessions::rename(&id, Some(&name))
    })
    .await?;

    info!(session = %session_id, title = %meta.title, "conversation renamed");
    emit_session(&state, &session_id).await;
    Ok(meta)
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct PermissionResponse {
    pub id: String,
    pub decision: PermissionDecision,
}

#[tauri::command]
pub async fn respond_permission(
    state: State<'_, Arc<AppState>>,
    response: PermissionResponse,
) -> CmdResult<()> {
    match state.pending_permissions.remove(&response.id) {
        Some((_, sender)) => {
            // Send failure means the waiting call already gave up; nothing to do.
            let _ = sender.send(response.decision);
            Ok(())
        }
        // Not an error: a turn that was canceled removes its own pending
        // requests, and the UI may answer a moment later.
        None => Ok(()),
    }
}

/// Answers a question card, releasing the tool call parked behind it.
///
/// One answer per question, in the order they were asked, and every one of them
/// may be empty — skipping is a first-class outcome, not a failure to respond.
/// See [`taurus_tools::view::Answer`].
#[tauri::command]
pub async fn answer_questions(
    state: State<'_, Arc<AppState>>,
    id: String,
    answers: Vec<Answer>,
) -> CmdResult<()> {
    match state.pending_questions.remove(&id) {
        Some((_, sender)) => {
            // Send failure means the call gave up first — a cancelled turn, or
            // a closed window. Nothing left to release.
            let _ = sender.send(answers);
            Ok(())
        }
        // Not an error, for the same reason `respond_permission` is not: a card
        // can still be on screen after the turn behind it was cancelled, and a
        // click on it should do nothing rather than raise a banner.
        None => Ok(()),
    }
}
