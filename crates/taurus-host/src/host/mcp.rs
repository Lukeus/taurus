//! MCP servers: their status, their layers, sign-in, and the reload that reconnects them.

use super::*;

impl Host {
    pub async fn mcp_statuses(&self) -> Vec<ServerStatus> {
        self.mcp.statuses().await
    }

    /// Every configured server, merged across layers, with how it is doing.
    ///
    /// One call rather than a listing plus a status lookup, because the two have
    /// to agree: a panel that renders a server from one snapshot and its state
    /// from another shows a connected server that is no longer configured for as
    /// long as it takes the second call to land.
    pub async fn mcp_servers(&self) -> Vec<McpServerView> {
        let workspace = self.workspace.read().await.clone();
        let (config, defined_in, problems) = self.mcp_layers(&workspace);
        // Recorded here as well as at a reload, because this is what the MCP
        // panel reads when it opens. A file broken by hand since the last
        // reload would otherwise take its servers off the list with nothing
        // anywhere saying why.
        self.replace_problems(ProblemSource::Mcp, problems).await;
        let statuses: BTreeMap<String, ServerStatus> = self
            .mcp
            .statuses()
            .await
            .into_iter()
            .map(|s| (s.name.clone(), s))
            .collect();

        // Read once for the whole list rather than per server: this assembles
        // every advertised tool's definition, and asking it back for each of a
        // dozen servers would rebuild that list a dozen times to answer the
        // same question.
        let names: Vec<String> = config.servers.keys().cloned().collect();
        let costs = mcp_schema_tokens(&names, &self.tool_definitions().await);

        config
            .servers
            .into_iter()
            .map(|(name, server)| {
                let status = statuses.get(&name).cloned();
                // Read here rather than inside the view, which is built from
                // config alone and has no way to reach a keychain.
                let signed_in = self.mcp.signed_in(&name);
                // Only for a server that actually connected. A disabled one
                // registers no tools, and reporting the zero that follows from
                // that would read as "this one is free".
                let schema_tokens = status
                    .as_ref()
                    .filter(|s| s.connected)
                    .map(|_| costs.get(&name).copied().unwrap_or(0));
                McpServerView {
                    signed_in,
                    schema_tokens,
                    ..McpServerView::new(name, server, &defined_in, status)
                }
            })
            .collect()
    }

    /// Begins a sign-in for one server, returning the URL to send a browser to.
    ///
    /// Two halves because the middle of it belongs to the application layer,
    /// which is the only one that can open a window. See `taurus_mcp::oauth`.
    pub async fn begin_mcp_sign_in(&self, name: &str) -> Result<taurus_mcp::oauth::SignIn, String> {
        let workspace = self.workspace.read().await.clone();
        let (config, _, _) = self.mcp_layers(&workspace);
        let server = config
            .servers
            .get(name)
            .ok_or_else(|| format!("'{name}' is not a configured server"))?;
        let taurus_mcp::ServerConfig::Http { url, .. } = server else {
            return Err(format!(
                "'{name}' talks over stdio, which takes its credentials from the \
                 environment rather than from a browser."
            ));
        };
        let url = taurus_tools::expand_env(url).map_err(|e| format!("url: {e}"))?;
        self.mcp.begin_sign_in(name, &url).await
    }

    /// Forgets one server's sign-in. Local only — the grant itself survives.
    pub fn mcp_sign_out(&self, name: &str) -> Result<(), String> {
        self.mcp.sign_out(name)
    }

    /// Both `mcp.json` layers, merged, plus which layer defined each server.
    ///
    /// The scope matters to the panel in a way it does not to the connector:
    /// editing a server has to write to the file it came from, and a workspace
    /// entry saved into the global file would silently change every other
    /// project.
    /// And what was wrong with either: the same problems [`Self::reload_mcp`]
    /// records, so a caller that reads the files can say what it read.
    pub(super) fn mcp_layers(
        &self,
        workspace: &Path,
    ) -> (taurus_mcp::McpConfig, LayerOf, Vec<Problem>) {
        let mut layers = Vec::new();
        let mut defined_in: LayerOf = BTreeMap::new();
        let mut problems = Vec::new();
        for scope in [Scope::Global, Scope::Workspace] {
            let Some(dir) = config::scope_dir(scope, Some(workspace)) else {
                continue;
            };
            let layer = match taurus_mcp::load(&dir) {
                Ok(layer) => layer,
                // Skipped, as a reload skips it, and said, as a reload says it.
                Err(e) => {
                    problems.push(Problem::new(ProblemSource::Mcp, e));
                    continue;
                }
            };
            for (name, server) in &layer.servers {
                // A toggle changes a server rather than defining one, so it must
                // not claim ownership: editing would then write a command line
                // into the file that only meant to switch one off.
                if !matches!(server, taurus_mcp::ServerConfig::Toggle(_)) {
                    defined_in.insert(name.clone(), scope);
                }
            }
            layers.push(layer);
        }
        let (merged, merge_problems) = config::merge_mcp(layers);
        problems.extend(Problem::tag(ProblemSource::Mcp, merge_problems));
        (merged, defined_in, problems)
    }

    /// Reconnects the MCP servers without rebuilding anything else.
    ///
    /// What the MCP panel calls after a save, and the second half of a
    /// [`Host::reload`]. It would also be done by a full reload, which would
    /// also rescan every skill directory and re-read both provider layers —
    /// none of which a change to `mcp.json` can affect. The narrower call is the
    /// same argument `rescan_agents` makes in the other direction: editing one
    /// thing should not restart the rest.
    ///
    /// The swap is by name. Every MCP tool carries the `mcp__` prefix, so the
    /// old set can be lifted out of the live registry and a new one put back
    /// without touching the built-ins, the skill tools, or the web tools beside
    /// them.
    pub async fn reload_mcp(&self) {
        // One at a time — see the field.
        let _reloading = self.mcp_reload.lock().await;
        let workspace = self.workspace.read().await.clone();

        let mut problems = Vec::new();
        let mut layers = Vec::new();
        for dir in config::config_dirs(Some(&workspace)) {
            match taurus_mcp::load(&dir) {
                Ok(layer) => layers.push(layer),
                // A layer that will not parse is skipped, not fatal: the other
                // one is still a working set of servers.
                Err(e) => problems.push(Problem::new(ProblemSource::Mcp, e)),
            }
        }
        let (config, merge_problems) = config::merge_mcp(layers);
        problems.extend(Problem::tag(ProblemSource::Mcp, merge_problems));

        // Out of the registry before the servers go down, rather than after
        // they come back. The other order left a turn that started in between
        // holding tools whose connections were already closed, and those fail
        // on every call; this way it sees no MCP tools, and works without.
        let before: HashSet<String> = {
            let mut registry = self.registry.write().await;
            let before: HashSet<String> = registry
                .names()
                .filter(|name| taurus_mcp::is_mcp_tool(name))
                .map(str::to_string)
                .collect();
            for name in &before {
                registry.remove(name);
            }
            before
        };

        // Reconnecting drops the previous connections, stopping the old child
        // processes; leaving them would leak one per workspace change.
        self.mcp.shutdown().await;
        let tools = self.mcp.connect_all(&config).await;

        // Applied to the new tools only. `reload_local` applies it to everything
        // else, and a tool the user turned off must not come back because its
        // server reconnected.
        let disabled = self.settings.read().await.disabled_tools.clone();
        let mut registry = self.registry.write().await;
        let mut after: HashSet<String> = HashSet::new();
        for tool in tools {
            if disabled.iter().any(|off| off == tool.name()) {
                continue;
            }
            after.insert(tool.name().to_string());
            registry.register(tool);
        }
        let available: Vec<String> = registry.names().map(str::to_string).collect();
        drop(registry);

        // Only this source. A malformed `providers.json` reported at the last
        // full reload is still malformed, and clearing it here would make it
        // vanish from Settings until something unrelated reloaded.
        self.replace_problems(ProblemSource::Mcp, problems).await;

        // The roster is checked against the tools that exist, so which MCP tools
        // exist is an input to it — and this is the only place that changes.
        //
        // Gated on the set actually moving rather than run every time, because
        // it is a rescan of every agent file and this runs on every save in the
        // MCP panel. It matters in both directions: an agent scoped only to a
        // server's tools is *refused* while that server is absent, so a startup
        // that checked the roster before connecting would have dropped it for
        // the session, and a server the user has just deleted leaves the roster
        // holding a tool that is gone.
        if before != after {
            let found = self.load_agents(&workspace, &available).await;
            self.replace_problems(ProblemSource::Agents, found).await;
        }
    }

    /// Connects to one server, reports what it offers, and disconnects.
    ///
    /// Nothing is registered and no live connection is disturbed — see
    /// [`taurus_mcp::probe`]. This is what makes "Test" safe to press against an
    /// edit of a server that is currently working.
    pub async fn test_mcp_server(
        &self,
        name: &str,
        server: &taurus_mcp::ServerConfig,
    ) -> Result<Vec<String>, String> {
        server.validate()?;
        // The same credentials the live connection would use, so Test tests
        // what actually runs rather than the unauthenticated half of it.
        taurus_mcp::probe(name, server, Some(Arc::new(crate::secrets::Keychain))).await
    }
}
