import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import { PermissionDialog } from "./components/PermissionDialog";
import { SkillProposalCard } from "./components/SkillProposalCard";
import { Transcript } from "./components/Transcript";
import * as api from "./lib/api";
import type { ModelInfo, SkillSummary } from "./lib/api";
import { useStore } from "./state/store";

export default function App() {
  const store = useStore();
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [skillsOpen, setSkillsOpen] = useState(false);

  useEffect(() => {
    store.init();
    // Intentionally once: init wires the event listeners.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const providerId = store.session?.provider_id ?? store.status?.providers[0]?.id;

  useEffect(() => {
    if (!providerId) return;
    api.listModels(providerId).then(setModels).catch(() => setModels([]));
  }, [providerId]);

  const pickWorkspace = async () => {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") await store.setWorkspace(chosen);
  };

  return (
    <div className="app">
      <header className="header">
        <button className="workspace" onClick={pickWorkspace} title="Change workspace">
          <span className="glyph">▤</span>
          {basename(store.status?.workspace ?? "…")}
        </button>

        <select
          className="model-select"
          value={store.session?.model ?? ""}
          disabled={store.busy || !providerId}
          onChange={(e) => providerId && store.startSession(providerId, e.target.value)}
        >
          {models.length === 0 && <option value="">no models</option>}
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.display_name}
            </option>
          ))}
        </select>

        {store.session && !store.session.native_tools && (
          <span className="tag warn" title="This model has no built-in tool calling; Taurus prompts for it instead.">
            prompted tools
          </span>
        )}

        <div className="spacer" />

        <button onClick={() => setSkillsOpen(true)}>
          Skills
          {store.status ? ` (${store.status.skill_count})` : ""}
        </button>
        <button onClick={store.clear} disabled={store.entries.length === 0}>
          Clear
        </button>
      </header>

      <main className="main">
        <Transcript entries={store.entries} busy={store.busy} />

        {store.proposals.length > 0 && (
          <div className="proposals">
            {store.proposals.map((p) => (
              <SkillProposalCard
                key={p.id}
                proposal={p}
                onResolve={(approve, target) =>
                  store.resolveProposal(p.id, approve, target)
                }
              />
            ))}
          </div>
        )}
      </main>

      <Composer
        busy={store.busy}
        ready={!!store.session}
        onSend={store.send}
        onStop={store.stop}
      />

      {store.error && (
        <div className="banner error">
          {store.error}
          <button onClick={store.dismissError}>dismiss</button>
        </div>
      )}

      {store.permission && (
        <PermissionDialog
          request={store.permission}
          onDecide={store.answerPermission}
        />
      )}

      {skillsOpen && <SkillsDrawer onClose={() => setSkillsOpen(false)} />}
    </div>
  );
}

function Composer({
  busy,
  ready,
  onSend,
  onStop,
}: {
  busy: boolean;
  ready: boolean;
  onSend: (text: string) => void;
  onStop: () => void;
}) {
  const [text, setText] = useState("");

  const submit = () => {
    if (!text.trim() || busy) return;
    onSend(text);
    setText("");
  };

  return (
    <footer className="composer">
      <textarea
        value={text}
        placeholder={ready ? "Ask Taurus to do something…" : "Select a model to begin"}
        disabled={!ready}
        rows={1}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          // Enter sends; Shift+Enter is a newline, matching every chat UI.
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            submit();
          }
        }}
      />
      {busy ? (
        <button className="danger" onClick={onStop}>
          Stop
        </button>
      ) : (
        <button className="primary" onClick={submit} disabled={!ready || !text.trim()}>
          Send
        </button>
      )}
    </footer>
  );
}

function SkillsDrawer({ onClose }: { onClose: () => void }) {
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const problems = useStore((s) => s.status?.skill_problems ?? []);
  const mcpServers = useStore((s) => s.status?.mcp_servers ?? []);

  useEffect(() => {
    api.listSkills().then(setSkills);
  }, []);

  return (
    <div className="scrim" onClick={onClose}>
      <aside className="drawer" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-head">
          <h2>Skills</h2>
          <button
            onClick={async () => {
              await api.reloadSkills();
              setSkills(await api.listSkills());
            }}
          >
            Rescan
          </button>
          <button onClick={onClose}>Close</button>
        </header>

        {skills.length === 0 && (
          <p className="muted drawer-empty">
            No skills yet. Taurus will offer to write one when it works out a
            procedure worth keeping.
          </p>
        )}

        <ul className="skill-list">
          {skills.map((skill) => (
            <li key={skill.name}>
              <div className="skill-row">
                <code>{skill.name}</code>
                <span className={`tag ${skill.tier}`}>{skill.tier}</span>
                {skill.degraded && (
                  <span className="tag warn" title={skill.degraded}>
                    scripts unavailable
                  </span>
                )}
              </div>
              <p className="muted">{skill.when_to_use}</p>
              {skill.scripts.length > 0 && (
                <p className="muted small">
                  {skill.scripts.map((s) => s.path).join(", ")}
                </p>
              )}
            </li>
          ))}
        </ul>

        {mcpServers.length > 0 && (
          <section className="mcp-servers">
            <h3>MCP servers</h3>
            {mcpServers.map((server) => (
              <div key={server.name} className="skill-row">
                <code>{server.name}</code>
                <span className={`tag ${server.connected ? "" : "warn"}`}>
                  {server.connected ? `${server.tool_count} tools` : "failed"}
                </span>
                {server.error && (
                  <span className="muted small" title={server.error}>
                    {server.error}
                  </span>
                )}
              </div>
            ))}
          </section>
        )}

        {problems.length > 0 && (
          <section className="skill-problems">
            <h3>Could not load</h3>
            {problems.map((p) => (
              <p key={p} className="small">
                {p}
              </p>
            ))}
          </section>
        )}
      </aside>
    </div>
  );
}

function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
