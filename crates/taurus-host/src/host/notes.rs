//! Memory notes, the notebook, and documents open on the canvas.

use super::*;

impl Host {
    /// What earlier conversations in this workspace left for the next one.
    ///
    /// Newest first, which is the order they are worth reading in and the order
    /// they reach the prompt. See [`crate::memory`].
    pub async fn notes(&self) -> Vec<memory::Note> {
        let mut notes = memory::load(&self.workspace.read().await.clone());
        notes.reverse();
        notes
    }

    /// Drops one, and returns what is left — in the same order [`Self::notes`]
    /// gives them, so a caller can redraw from the answer.
    pub async fn forget_note(&self, id: &str) -> Result<Vec<memory::Note>, String> {
        let mut left = memory::forget(&self.workspace.read().await.clone(), id)?;
        left.reverse();
        Ok(left)
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
        notebook::list(Some(&self.workspace().await))
    }

    pub async fn read_page(
        &self,
        scope: Scope,
        kind: notebook::PageKind,
        name: &str,
    ) -> Result<notebook::Page, String> {
        notebook::read(scope, Some(&self.workspace().await), kind, name)
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
        notebook::save(
            scope,
            Some(&self.workspace().await),
            kind,
            name,
            text,
            fingerprint,
        )
    }

    pub async fn create_page(
        &self,
        scope: Scope,
        kind: notebook::PageKind,
        name: &str,
    ) -> Result<notebook::Page, String> {
        notebook::create(scope, Some(&self.workspace().await), kind, name)
    }

    pub async fn rename_page(
        &self,
        scope: Scope,
        kind: notebook::PageKind,
        name: &str,
        to: &str,
    ) -> Result<notebook::Page, String> {
        notebook::rename(scope, Some(&self.workspace().await), kind, name, to)
    }

    pub async fn forget_page(
        &self,
        scope: Scope,
        kind: notebook::PageKind,
        name: &str,
    ) -> Result<Vec<notebook::PageRef>, String> {
        notebook::forget(scope, Some(&self.workspace().await), kind, name)
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
            let resolved =
                taurus_tools::path_guard::resolve(&workspace, &path).map_err(|e| e.to_string())?;
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
        crate::document::save(&workspace, path, text, fingerprint)
    }
}
