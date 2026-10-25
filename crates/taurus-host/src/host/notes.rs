//! Memory notes, the notebook, and documents open on the canvas.

use super::*;

impl Host {
    /// What earlier conversations in this workspace left for the next one.
    ///
    /// Newest first, which is the order they are worth reading in and the order
    /// they reach the prompt. See [`crate::memory`].
    pub async fn notes(&self) -> Vec<memory::Note> {
        let workspace = self.workspace.read().await.clone();
        let mut notes = off_runtime_or_default(move || memory::load(&workspace)).await;
        notes.reverse();
        notes
    }

    /// Drops one, and returns what is left — in the same order [`Self::notes`]
    /// gives them, so a caller can redraw from the answer.
    pub async fn forget_note(&self, id: &str) -> Result<Vec<memory::Note>, String> {
        let (workspace, id) = (self.workspace.read().await.clone(), id.to_string());
        let mut left = off_runtime("forgetting the note", move || {
            memory::forget(&workspace, &id)
        })
        .await?;
        left.reverse();
        Ok(left)
    }

    /// Writes a note of your own, read into every later conversation here as
    /// the model's are. Answers with every note, newest first. See
    /// [`memory::BY_YOU`].
    pub async fn add_note(&self, text: &str) -> Result<Vec<memory::Note>, String> {
        let (workspace, text) = (self.workspace().await, text.to_string());
        let mut notes = off_runtime("writing the note", move || {
            memory::append(&workspace, memory::BY_YOU, &text)
        })
        .await?;
        notes.reverse();
        Ok(notes)
    }

    /// Changes one note's text. Answers with every note, newest first.
    pub async fn edit_note(&self, id: &str, text: &str) -> Result<Vec<memory::Note>, String> {
        let (workspace, id, text) = (self.workspace().await, id.to_string(), text.to_string());
        let mut notes = off_runtime("changing the note", move || {
            memory::edit(&workspace, &id, &text)
        })
        .await?;
        notes.reverse();
        Ok(notes)
    }

    /* ----------------------------------------------------------- notebook */

    /// Every note somebody has written, in both scopes.
    ///
    /// A different thing entirely from [`Self::notes`] one screen up, and the
    /// two are worth telling apart: those are written by the model, capped, and
    /// read into the next conversation's prompt. These are written by a person,
    /// live in `.taurus/notes/` where they can be committed, and reach the model
    /// only when somebody asks about one.
    pub async fn notebook(&self) -> Vec<notebook::PageRef> {
        let workspace = self.workspace().await;
        off_runtime_or_default(move || notebook::list(Some(&workspace))).await
    }

    pub async fn read_page(
        &self,
        scope: Scope,
        kind: notebook::PageKind,
        name: &str,
    ) -> Result<notebook::Page, String> {
        let (workspace, name) = (self.workspace().await, name.to_string());
        off_runtime("reading the note", move || {
            notebook::read(scope, Some(&workspace), kind, &name)
        })
        .await
    }

    /// Writes a note, unless it has changed since the editor read it.
    ///
    /// The same guarantee the canvas has and the same implementation of it — see
    /// [`crate::document::write_if_current`].
    pub async fn save_page(
        &self,
        scope: Scope,
        kind: notebook::PageKind,
        name: &str,
        text: &str,
        fingerprint: &str,
    ) -> Result<notebook::PageSaved, String> {
        let workspace = self.workspace().await;
        let (name, text, fingerprint) =
            (name.to_string(), text.to_string(), fingerprint.to_string());
        off_runtime("saving the note", move || {
            notebook::save(scope, Some(&workspace), kind, &name, &text, &fingerprint)
        })
        .await
    }

    pub async fn create_page(
        &self,
        scope: Scope,
        kind: notebook::PageKind,
        name: &str,
    ) -> Result<notebook::Page, String> {
        let (workspace, name) = (self.workspace().await, name.to_string());
        off_runtime("creating the note", move || {
            notebook::create(scope, Some(&workspace), kind, &name)
        })
        .await
    }

    pub async fn rename_page(
        &self,
        scope: Scope,
        kind: notebook::PageKind,
        name: &str,
        to: &str,
    ) -> Result<notebook::Page, String> {
        let (workspace, name, to) = (self.workspace().await, name.to_string(), to.to_string());
        off_runtime("renaming the note", move || {
            notebook::rename(scope, Some(&workspace), kind, &name, &to)
        })
        .await
    }

    pub async fn forget_page(
        &self,
        scope: Scope,
        kind: notebook::PageKind,
        name: &str,
    ) -> Result<Vec<notebook::PageRef>, String> {
        let (workspace, name) = (self.workspace().await, name.to_string());
        off_runtime("deleting the note", move || {
            notebook::forget(scope, Some(&workspace), kind, &name)
        })
        .await
    }

    /* ------------------------------------------------------------- canvas */

    /// One text file, read for the editor.
    ///
    /// The canvas reads through here rather than being handed the text by the
    /// tool that opened it, and that separation is the point. `open_file` says
    /// *which* file; this says what is in it *now*. So a card in a conversation
    /// from last week opens today's version, a file the model has just
    /// rewritten reloads by calling this again, and the editor never shows
    /// bytes that came from somewhere other than the disk.
    ///
    /// Guarded like every other read: the path comes from a transcript the
    /// user may have reopened, or from a click, and neither is a reason to let
    /// `../` out of the workspace.
    pub async fn open_document(&self, path: &str) -> Result<Document, String> {
        let workspace = self.workspace().await;
        let path = path.to_string();
        // On a blocking thread: a document is read whole, up to
        // `MAX_DOCUMENT_BYTES`, and the canvas opens one beside a live turn.
        tokio::task::spawn_blocking(move || {
            // The workspace, and the library's personal folders: a person
            // changing a skill they use everywhere opens it here too. Never
            // widened for a tool. See `crate::library`.
            let also = crate::library::editable_roots();
            let resolved = taurus_tools::path_guard::resolve_within(&workspace, &also, &path)
                .map_err(|e| e.to_string())?;
            let shown = taurus_tools::path_guard::display(&workspace, &resolved);

            let meta =
                std::fs::metadata(&resolved).map_err(|e| format!("Could not open {shown}: {e}"))?;
            if meta.is_dir() {
                return Err(format!("{shown} is a folder, not a file."));
            }
            if meta.len() > MAX_DOCUMENT_BYTES {
                return Err(format!(
                    "{shown} is too large to open in the editor ({:.1} MB). Files up to {} MB \
                     open here; anything bigger is better read in pieces.",
                    meta.len() as f64 / (1024.0 * 1024.0),
                    MAX_DOCUMENT_BYTES / (1024 * 1024)
                ));
            }

            let text = std::fs::read_to_string(&resolved).map_err(|e| {
                if e.kind() == std::io::ErrorKind::InvalidData {
                    format!(
                        "{shown} is not a text file, so there is nothing to show in the editor."
                    )
                } else {
                    format!("Could not open {shown}: {e}")
                }
            })?;

            Ok(Document {
                lines: text.lines().count() as u32,
                fingerprint: fingerprint(&meta),
                path: shown,
                text,
            })
        })
        .await
        .map_err(|e| format!("reading the file failed: {e}"))?
    }

    /// Writes what the editor holds, unless the file moved since it was read.
    ///
    /// No permission prompt, and that is the decision rather than an omission:
    /// this is a person typing in an editor they opened, not the model changing
    /// a file. See [`crate::document`] for the argument and for what follows
    /// from it — chiefly that a canvas edit does not appear in the Changes
    /// drawer, which is about what the *conversation* changed.
    pub async fn save_document(
        &self,
        path: &str,
        text: &str,
        fingerprint: &str,
    ) -> Result<Saved, String> {
        let workspace = self.workspace().await;
        let (path, text, fingerprint) =
            (path.to_string(), text.to_string(), fingerprint.to_string());
        off_runtime("saving the file", move || {
            let also = crate::library::editable_roots();
            crate::document::save(&workspace, &also, &path, &text, &fingerprint)
        })
        .await
    }
}
