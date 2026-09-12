//! The open folder and the trust decision about it.

use super::*;

impl Host {
    pub async fn set_workspace(&self, path: &Path) -> Result<PathBuf, String> {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !canonical.is_dir() {
            return Err(format!(
                "{} is not a directory",
                taurus_tools::path_guard::plain(&canonical).display()
            ));
        }

        // The index being built is this workspace's, and the next line makes
        // it the wrong one.
        self.indexing.stop();

        // A background command belongs to the workspace it was started in: its
        // cwd is about to stop meaning what it meant, and its changes would be
        // swept against a workspace it never ran in.
        self.jobs.forget_all();
        // What the last workspace's commands read is keyed by its paths and
        // useless here, and holding it would keep that workspace in memory.
        self.sweeps.clear();

        *self.workspace.write().await = canonical.clone();
        // Rebuilt with this workspace's trust state, so a committed allowlist
        // in a directory the user has not vouched for is not consulted and
        // "always allow here" is not offered.
        self.rebuild_permissions(&canonical).await;

        // Global, and only global: "the workspace I had open" is a fact about
        // the user, and writing it into the workspace it names would be a file
        // that can only ever point at its own directory.
        // Stored plain. It is read back and canonicalized again on the next
        // start, so the verbatim form buys nothing and is what a person opening
        // the settings file would have to read past.
        let remembered = taurus_tools::path_guard::plain(&canonical)
            .display()
            .to_string();
        config::edit_settings(Scope::Global, None, |s| s.last_workspace = Some(remembered));

        // Reload re-resolves both layers, so the in-memory settings pick up the
        // new workspace's file without a second write. The local half only: the
        // new folder's servers are the caller's to reconnect, and not to wait
        // for — a folder with three `npx` servers would otherwise hold the
        // window on the old folder until the last of them answered.
        self.reload_local().await;
        Ok(canonical)
    }

    /// Whether this workspace's own config is being read, and what it holds.
    ///
    /// Cheap enough to call whenever a frontend redraws: it is a `stat` of each
    /// project-tier file and a parse of the two that have entries worth naming.
    /// Nothing here loads a skill or starts a server — describing the decision
    /// must not do the thing the decision governs. See [`crate::trust`].
    pub async fn trust_status(&self) -> crate::trust::TrustStatus {
        crate::trust::status(&self.workspace.read().await)
    }

    /// Lets this workspace's config take effect, now and from now on.
    ///
    /// Reloads rather than waiting for the next turn, because the user just
    /// answered a question about what this project contributes and the honest
    /// response is to contribute it. That is also what rebuilds the permission
    /// engine — the workspace allowlist was not read at startup, and there is
    /// no other moment it would be picked up.
    ///
    /// The local half of a reload, as [`Self::set_workspace`] does: the servers
    /// this project names are the caller's to start, after it has redrawn.
    pub async fn trust_workspace(&self) -> Result<(), String> {
        let workspace = self.workspace.read().await.clone();
        crate::trust::trust(&workspace)?;
        self.rebuild_permissions(&workspace).await;
        self.reload_local().await;
        Ok(())
    }

    /// Stops reading this workspace's config.
    ///
    /// The reload is what makes it take effect immediately: a skill loaded
    /// under the old decision is dropped from the catalog before this returns.
    /// An MCP server started under it is shut down by the caller's reconnect,
    /// which it starts once it has redrawn — see [`Self::trust_workspace`].
    pub async fn revoke_trust(&self) -> Result<(), String> {
        let workspace = self.workspace.read().await.clone();
        crate::trust::revoke(&workspace)?;
        self.rebuild_permissions(&workspace).await;
        self.reload_local().await;
        Ok(())
    }

    /// Rebuilds the permission engine against the current trust decision.
    ///
    /// The engine reads the workspace allowlist once, at construction, so a
    /// trust decision made after that has no effect until it is built again.
    /// Prompts in flight are unaffected: each holds its own channel, and this
    /// replaces the engine the *next* call will consult.
    pub(super) async fn rebuild_permissions(&self, workspace: &Path) {
        *self.permissions.write().await = Arc::new(
            PermissionEngine::new(workspace, config::home_dir(), self.prompts.create())
                .with_workspace_rules(crate::trust::is_trusted(workspace)),
        );
    }
}
