import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { McpEnvironment, McpServerView, Scope } from "../lib/api";
import { plural } from "../lib/format";
import { stateOf } from "../lib/mcp";
import { McpServerEditor } from "./McpServerEditor";
import { useStore } from "../state/store";
import { Modal } from "./Modal";

/**
 * Every tool server this workspace reaches, and everything needed to work out
 * why one is not answering.
 *
 * This used to be a section at the bottom of the Skills drawer holding a status
 * line and a link to the file. That was enough when the only failure was a
 * server that was genuinely misconfigured, and not enough for the one that
 * actually happens: an entry that is correct, a program that is installed, and a
 * window that cannot see it because it was started by the Dock rather than by a
 * shell. Diagnosing that needs three things the old section did not have — the
 * PATH being searched, whether the command resolves on it, and a way to try a
 * server without waiting for a reload.
 *
 * The file is still the authority. Everything written from here goes through an
 * edit that preserves what it does not understand, so a server added in the form
 * and one pasted into `mcp.json` are the same server, and neither route
 * disturbs the other.
 */
export function McpDrawer({ onClose }: { onClose: () => void }) {
  const [servers, setServers] = useState<McpServerView[] | null>(null);
  const [environment, setEnvironment] = useState<McpEnvironment | null>(null);
  const [editing, setEditing] = useState<McpServerView | "new" | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  /*
   * One stable slice, narrowed here rather than in the selector — the same note
   * as in SkillsDrawer. zustand v5 compares snapshots with `Object.is`, and a
   * selector ending in `.filter(…)` allocates a fresh array per call, which
   * stops the drawer opening at all.
   */
  const status = useStore((s) => s.status);
  const refreshStatus = useStore((s) => s.refresh);
  // Entries that would not parse have no server to hang a failed status off, so
  // this list is the only place they can appear.
  const problems = (status?.problems ?? []).filter((p) => p.source === "mcp");

  const load = async () => {
    const [found, env] = await Promise.all([
      api.listMcpServers(),
      api.mcpEnvironment(),
    ]);
    setServers(found);
    setEnvironment(env);
  };

  useEffect(() => {
    load().catch((e) => {
      setError(String(e));
      setServers([]);
    });
    // Once, on open. Reading the current state is the point of mounting.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /**
   * Runs a write, takes the listing it returns, and refreshes the rail.
   *
   * Every write command answers with the listing after it, so the panel never
   * follows a save with a read that could disagree with it. The store refresh is
   * for the count beside the rail entry, which comes from a different snapshot.
   */
  const apply = async (write: () => Promise<McpServerView[]>) => {
    setBusy(true);
    setError(null);
    try {
      setServers(await write());
      await refreshStatus();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const connected = (servers ?? []).filter((s) => s.status?.connected).length;
  const tools = (servers ?? []).reduce(
    (sum, s) => sum + (s.status?.connected ? s.status.tool_count : 0),
    0,
  );

  return (
    <Modal onClose={onClose}>
      <aside className="drawer" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-head">
          <h2>MCP servers</h2>
          {/* Reconnect rather than Rescan: this restarts programs, which is a
              heavier thing than rereading a directory and should say so. */}
          <button
            disabled={busy}
            onClick={() => apply(api.reloadMcp)}
            data-tip="Restart every server and ask each one what it offers"
          >
            {busy ? "Connecting…" : "Reconnect"}
          </button>
          <button className="drawer-close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>

        <p className="drawer-intro">
          External tool servers. Everything they offer becomes a tool Taurus can
          call, under the same permission prompt as a built-in.
        </p>

        {/* Between reading and having read nothing: this drawer showed the
            same blank for both, so a slow read looked like an empty file.
            `MemoryDrawer` is where this line comes from. */}
        {servers === null && <p className="drawer-loading">Reading…</p>}

        {servers !== null && servers.length > 0 && (
          <p className="drawer-intro hint">
            {connected === servers.length
              ? `All ${plural(servers.length, "server")} connected · ${plural(tools, "tool")}`
              : `${connected} of ${servers.length} connected · ${plural(tools, "tool")}`}
          </p>
        )}

        {error && <p className="settings-problem">{error}</p>}

        {servers !== null && servers.length === 0 && (
          <p className="drawer-empty">
            No servers configured yet. Add one below, or paste an existing
            <code> mcpServers</code> block into mcp.json — the format is the one
            Claude Desktop and Claude Code use.
          </p>
        )}

        <ul className="card-list">
          {(servers ?? []).map((server) => (
            <ServerCard
              key={`${server.scope}:${server.name}`}
              server={server}
              busy={busy}
              onEdit={() => setEditing(server)}
              onToggle={() =>
                apply(() =>
                  api.setMcpServerDisabled(
                    server.scope,
                    server.name,
                    !server.disabled,
                  ),
                )
              }
              onDelete={() =>
                apply(() => api.deleteMcpServer(server.scope, server.name))
              }
            />
          ))}
        </ul>

        {problems.length > 0 && (
          <section className="section">
            <span className="micro">Could not read</span>
            {problems.map((problem) => (
              <p key={problem.message} className="settings-problem">
                {problem.message}
              </p>
            ))}
          </section>
        )}

        <PathSection environment={environment} servers={servers ?? []} />

        <div className="drawer-foot">
          <button className="card-add" onClick={() => setEditing("new")}>
            Add server — stdio or HTTP
          </button>
          {/* Still here, and deliberately. The panel writes the same file, and
              anything it cannot express is edited in the one place that can. */}
          <button
            className="link"
            onClick={async () => {
              try {
                await api.openMcpConfig("global");
              } catch (e) {
                setError(String(e));
              }
            }}
          >
            Edit mcp.json
          </button>
        </div>
      </aside>

      {editing && (
        <McpServerEditor
          server={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={(updated) => {
            setEditing(null);
            setServers(updated);
            refreshStatus().catch(() => {});
          }}
        />
      )}
    </Modal>
  );
}

/**
 * One server, as the list shows it.
 *
 * The order answers what someone scanning this actually asks: what is it, is it
 * working, and — when it is not — the one fact that explains why. The tool list
 * comes last because it is what you read once everything is fine.
 */
function ServerCard({
  server,
  busy,
  onEdit,
  onToggle,
  onDelete,
}: {
  server: McpServerView;
  busy: boolean;
  onEdit: () => void;
  onToggle: () => void;
  onDelete: () => void;
}) {
  /*
   * Deleting takes an entry someone typed, and nothing brings it back. The
   * button arms rather than acts, the same way the conversation list's does.
   */
  const [arming, setArming] = useState(false);
  const [showTools, setShowTools] = useState(false);
  const state = stateOf(server);
  const stdio = server.transport === "stdio";
  // The single most useful line on a failed stdio server, and the reason this
  // panel exists: the command is spelled correctly and the app cannot see it.
  const missing = stdio && server.command.trim() !== "" && !server.program;

  return (
    <li className={`card mcp${server.disabled ? " off" : ""}`}>
      <div className="card-body">
        <div className="card-row">
          <span className={`dot ${state.tone === "ok" ? "ok" : state.tone === "error" ? "error" : ""}`} />
          <span className="card-title">{server.name}</span>
          <span className="tag">{stdio ? "stdio" : "http"}</span>
          <span className={`tag ${server.scope === "workspace" ? "project" : ""}`}>
            {server.scope === "workspace" ? "project" : "all projects"}
          </span>
          {server.disabled && <span className="tag warn">off</span>}
        </div>

        <span className="card-sub mono">
          {stdio
            ? [server.command, ...server.args].join(" ").trim() || "(no command)"
            : server.url || "(no url)"}
        </span>

        {/* Where the program actually is, or that it is nowhere. Shown before a
            connect is attempted, so the answer is there the first time someone
            looks rather than after a reload. */}
        {stdio && server.program && (
          <span className="card-files mono">→ {server.program}</span>
        )}
        {/* One line, not two. When the program is missing the connect error says
            the same thing at more length, and a card that states one fact twice
            reads as two problems. The shorter line wins because it points at the
            search path below, which is the part that is actionable. */}
        {missing ? (
          <span className="card-files warn">
            {server.command} is not on Taurus's PATH — see below, or give the
            full path to the program.
          </span>
        ) : (
          state.tone === "error" && (
            <span className="card-files warn">{state.label}</span>
          )
        )}
        {state.tone === "unknown" && (
          <span className="card-files">Not connected yet — press Reconnect.</span>
        )}

        {server.status?.connected && server.status.tools.length > 0 && (
          <>
            <button
              className="link tools-toggle"
              onClick={() => setShowTools((open) => !open)}
            >
              {showTools ? "Hide" : "Show"}{" "}
              {plural(server.status.tool_count, "tool")}
            </button>
            {showTools && (
              <div className="tool-chips">
                {server.status.tools.map((tool) => (
                  <span key={tool} className="tool-chip mono">
                    {tool}
                  </span>
                ))}
              </div>
            )}
          </>
        )}

        <div className="card-actions">
          <button disabled={busy} onClick={onEdit}>
            Edit
          </button>
          <button disabled={busy} onClick={onToggle}>
            {server.disabled ? "Enable" : "Disable"}
          </button>
          {arming ? (
            <>
              <button
                className="danger"
                disabled={busy}
                onClick={() => {
                  setArming(false);
                  onDelete();
                }}
              >
                Remove from mcp.json
              </button>
              <button disabled={busy} onClick={() => setArming(false)}>
                Keep
              </button>
            </>
          ) : (
            <button disabled={busy} onClick={() => setArming(true)}>
              Delete
            </button>
          )}
        </div>
      </div>
    </li>
  );
}

/**
 * The PATH the app searches, and what it took to get it.
 *
 * Collapsed by default, because on a healthy setup it is a list of directories
 * nobody needs — and opened by default when something above it is missing, which
 * is exactly when it is the answer. A window started from the Dock inherits the
 * launcher's PATH, so Taurus asks the login shell for the real one at startup;
 * this says whether that worked and what it added.
 */
export function PathSection({
  environment,
  servers,
}: {
  environment: McpEnvironment | null;
  servers: McpServerView[];
}) {
  const missing = servers.filter(
    (s) => s.transport === "stdio" && s.command.trim() !== "" && !s.program,
  );
  const [open, setOpen] = useState(false);
  // Opened for the case it explains, rather than left for someone to find.
  useEffect(() => {
    if (missing.length > 0) setOpen(true);
  }, [missing.length]);

  if (!environment) return null;

  return (
    <section className="section">
      <div className="section-head">
        <span className="micro">Program search path</span>
        <button className="link" onClick={() => setOpen((o) => !o)}>
          {open ? "Hide" : `${environment.path.length} directories`}
        </button>
      </div>

      {environment.skipped ? (
        <p className="hint">
          Taurus did not read your login shell's PATH ({environment.skipped}), so
          it searches only what it was started with.
        </p>
      ) : environment.added.length > 0 ? (
        <p className="hint">
          Taurus read your login shell's PATH at startup and added{" "}
          {plural(environment.added.length, "directory", "directories")} the
          launcher did not have.
        </p>
      ) : (
        <p className="hint">
          Started with a complete PATH — your login shell had nothing to add.
        </p>
      )}

      {open && (
        <div className="path-list mono">
          {environment.path.map((dir) => (
            <div
              key={dir}
              className={environment.added.includes(dir) ? "added" : undefined}
            >
              {dir}
            </div>
          ))}
        </div>
      )}

      {missing.length > 0 && (
        <p className="hint warn">
          {missing.map((s) => s.command).join(", ")} could not be found here. If
          it was installed after Taurus started, restart Taurus; otherwise give
          the full path to the program as the command.
        </p>
      )}
    </section>
  );
}

export type { Scope };
