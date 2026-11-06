//! Memory notes, the notebook, and documents open on the canvas.

use super::*;
use crate::WorkspaceFiles;

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

    /* ----------------------------------------------------- sketch library */

    /// The shapes kept in the sketch editor's library, as the file holds them,
    /// or `None` before anything has been added.
    ///
    /// One library for every sketch in every folder, the way Excalidraw's own
    /// is: a shape somebody keeps is theirs, not a project's. It's a file in
    /// `~/.taurus` rather than the webview's storage because it's work, not a
    /// preference, and clearing site data shouldn't take it. It's Excalidraw's
    /// own `.excalidrawlib` format, so a library downloaded from the public
    /// site can be dropped in place of it, and this one can be shared the same
    /// way.
    pub async fn sketch_library(&self) -> Result<Option<String>, String> {
        let path = sketch_library_path();
        off_runtime(
            "reading the sketch library",
            move || match std::fs::read_to_string(&path) {
                Ok(text) => Ok(Some(text)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(format!("Couldn't read {}: {e}", path.display())),
            },
        )
        .await
    }

    /// Replaces the sketch library with `text`, which has to be a library.
    ///
    /// Checked before it's written, because the editor writes this on every
    /// change to the library, and a value that wasn't one would replace the
    /// shapes somebody kept with something nothing can read. Written whole and
    /// atomically, like every config file.
    pub async fn save_sketch_library(&self, text: &str) -> Result<(), String> {
        check_sketch_library(text)?;
        let (path, text) = (sketch_library_path(), text.to_string());
        off_runtime("saving the sketch library", move || {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
            }
            config::replace_file(&path, &text)
                .map_err(|e| format!("Couldn't save {}: {e}", path.display()))
        })
        .await
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

    /// Every file in the workspace, for the file finder.
    ///
    /// Capped, because the list crosses the IPC channel as one message. Only ever
    /// the workspace: the library's personal folders have their own drawers.
    pub async fn workspace_files(&self) -> Result<WorkspaceFiles, String> {
        const MOST: usize = 50_000;
        const LONGEST: std::time::Duration = std::time::Duration::from_secs(5);
        let workspace = self.workspace().await;
        tokio::task::spawn_blocking(move || {
            let (paths, cut) = taurus_tools::builtin::search::list_files(&workspace, MOST, LONGEST)
                .map_err(|e| format!("Could not list the files in this workspace: {e}"))?;
            Ok(WorkspaceFiles { paths, cut })
        })
        .await
        .map_err(|e| format!("listing the files failed: {e}"))?
    }

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

/// Where the sketch library is kept. See [`Host::sketch_library`].
pub(crate) fn sketch_library_path() -> std::path::PathBuf {
    config::home_dir().join(SKETCH_LIBRARY_FILE)
}

/// The library's file name, beside `settings.json`.
pub const SKETCH_LIBRARY_FILE: &str = "sketch-library.excalidrawlib";

/// The largest library that's written. A shape is a few kilobytes, so this is
/// thousands of them. Past it, something is writing that isn't a person adding
/// shapes.
const MAX_SKETCH_LIBRARY_BYTES: usize = 16 * 1024 * 1024;

/// Whether `text` is something the library file may hold: Excalidraw's
/// `excalidrawlib` document, with its shapes as a list.
fn check_sketch_library(text: &str) -> Result<(), String> {
    if text.len() > MAX_SKETCH_LIBRARY_BYTES {
        return Err(format!(
            "The sketch library wasn't saved: at {} MB it's over the {} MB limit.",
            text.len().div_ceil(1024 * 1024),
            MAX_SKETCH_LIBRARY_BYTES / (1024 * 1024)
        ));
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("The sketch library wasn't saved: it isn't JSON ({e})."))?;
    let is_library = value.get("type").and_then(|t| t.as_str()) == Some("excalidrawlib")
        && value
            .get("libraryItems")
            .is_some_and(|items| items.is_array());
    if !is_library {
        return Err(
            "The sketch library wasn't saved: it isn't an Excalidraw library \
             (`\"type\": \"excalidrawlib\"` with a `libraryItems` list)."
                .into(),
        );
    }
    Ok(())
}
