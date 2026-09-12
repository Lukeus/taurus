//! Memory notes, the notebook, and documents on the canvas.

use super::*;

/// What earlier conversations in this workspace wrote down for the next one.
///
/// Shown for the same reason the standing brief is: this is part of what is in
/// the model's context before a conversation starts, and context the user
/// cannot see is behaviour they cannot explain. Unlike the brief, they can also
/// take a line out of it.
#[tauri::command]
pub async fn list_notes(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<Note>> {
    Ok(state.host.notes().await)
}

/// Every note and sketch somebody has written here, in both scopes.
///
/// Not the same thing as `list_notes` above, and the difference is worth keeping
/// straight: those are the model's, capped and read into the next conversation's
/// prompt. These are the person's, and they are Markdown files in
/// `.taurus/notes/` that get committed with the project.
#[tauri::command]
pub async fn list_pages(state: State<'_, Arc<AppState>>) -> CmdResult<Vec<PageRef>> {
    Ok(state.host.notebook().await)
}

/// One note, read off disk.
///
/// Called when a note is opened and again when the pane comes back to it. The
/// list carries no text, for the reason a `show_table` card carries no rows: a
/// remembered copy is confidently wrong the moment anything else writes the
/// file.
#[tauri::command]
pub async fn read_page(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    kind: PageKind,
    name: String,
) -> CmdResult<Page> {
    state.host.read_page(scope, kind, &name).await
}

/// Writes what the editor holds back to the note.
///
/// `fingerprint` is the one the editor was handed when it read, and the whole of
/// the guarantee — the same one the canvas has, and the same code. A save that
/// does not match what is on disk writes nothing and comes back as
/// `PageSaved::Stale` carrying the other version, which is not an error.
#[tauri::command]
pub async fn save_page(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    kind: PageKind,
    name: String,
    text: String,
    fingerprint: String,
) -> CmdResult<PageSaved> {
    state
        .host
        .save_page(scope, kind, &name, &text, &fingerprint)
        .await
}

/// Starts a note, refusing a name that is taken or cannot be a filename.
#[tauri::command]
pub async fn create_page(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    kind: PageKind,
    name: String,
) -> CmdResult<(Page, Vec<PageRef>)> {
    let page = state.host.create_page(scope, kind, &name).await?;
    // With the list it now belongs to, so the pane redraws from one answer
    // rather than asking for the list again — the bargain `forget_page` makes.
    Ok((page, state.host.notebook().await))
}

/// Renames a note, which moves its file: the name *is* the filename. Answers
/// with the list as well, for the reason `create_page` does.
#[tauri::command]
pub async fn rename_page(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    kind: PageKind,
    name: String,
    to: String,
) -> CmdResult<(Page, Vec<PageRef>)> {
    let page = state.host.rename_page(scope, kind, &name, &to).await?;
    Ok((page, state.host.notebook().await))
}

/// Deletes a note and gives back what is left, so the pane can redraw from the
/// answer rather than asking again.
#[tauri::command]
pub async fn forget_page(
    state: State<'_, Arc<AppState>>,
    scope: Scope,
    kind: PageKind,
    name: String,
) -> CmdResult<Vec<PageRef>> {
    state.host.forget_page(scope, kind, &name).await
}

/// One text file, read for the canvas.
///
/// Called when a document is opened and again whenever it has to be re-read —
/// the model rewrote it, the window came back into focus. Cheap enough to be
/// the answer to both: a file small enough for an editor is one `read`.
///
/// Not the tool's job to supply this, deliberately. See `taurus_host::document`
/// for why a card carries a path and never a copy of the file.
#[tauri::command]
pub async fn open_document(state: State<'_, Arc<AppState>>, path: String) -> CmdResult<Document> {
    state.host.open_document(&path).await
}

/// Writes what the canvas holds back to the file.
///
/// `fingerprint` is the one the editor was handed when it read, and the whole
/// of the guarantee: a save that does not match what is on disk writes nothing
/// and comes back as `Saved::Stale` carrying the other version. That is not an
/// error — see `taurus_host::document`.
#[tauri::command]
pub async fn save_document(
    state: State<'_, Arc<AppState>>,
    path: String,
    text: String,
    fingerprint: String,
) -> CmdResult<Saved> {
    state.host.save_document(&path, &text, &fingerprint).await
}

/// Drops one note and answers with what is left, so the drawer redraws from the
/// file rather than from its own guess about what the file now says.
#[tauri::command]
pub async fn forget_note(state: State<'_, Arc<AppState>>, id: String) -> CmdResult<Vec<Note>> {
    let left = state.host.forget_note(&id).await?;
    // The drawer is handed the remaining notes directly; this is for the count
    // on the rail behind it.
    emit_status(&state).await;
    Ok(left)
}
