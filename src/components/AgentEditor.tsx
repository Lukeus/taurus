import { useEffect, useState } from "react";

import * as api from "../lib/api";
import type { AgentProposal } from "../lib/api";
import {
  DEFAULT_AGENT_ITERATIONS,
  DESCRIPTION_LIMIT,
  MAX_ITERATIONS_LIMIT,
} from "../lib/limits";
import { useStore } from "../state/store";
import { Modal } from "./Modal";

/**
 * Writing an agent without already knowing the frontmatter.
 *
 * The file stays the source of truth — this writes a `.md` through the same
 * validation and the same writer an approved proposal takes, so an agent
 * written here and one written in a text editor are the same thing. What this
 * adds is that the format is discoverable: every constraint the loader enforces
 * (kebab-case, the description limit, the iteration ceiling, tools that exist
 * here)
 * is visible in the form rather than found by having a save rejected.
 *
 * Generate is a starting point, not a result. It fills the same boxes the user
 * would have typed into, and every one stays editable — which is what keeps
 * this an editor with a draft button rather than a wizard.
 */
export function AgentEditor({
  onClose,
  onSaved,
}: {
  onClose: () => void;
  onSaved: () => void | Promise<void>;
}) {
  const session = useStore((s) => s.session);

  const [ask, setAsk] = useState("");
  const [generating, setGenerating] = useState(false);

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [prompt, setPrompt] = useState("");
  const [maxIterations, setMaxIterations] = useState(DEFAULT_AGENT_ITERATIONS);
  /** `null` is inherit-the-caller's-tools, which is not the same as none. */
  const [tools, setTools] = useState<string[] | null>(null);
  const [target, setTarget] = useState<"project" | "user">("project");

  const [available, setAvailable] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.listTools().then(setAvailable).catch(() => setAvailable([]));
  }, []);

  const apply = (draft: AgentProposal) => {
    setName(draft.name);
    setDescription(draft.description);
    setPrompt(draft.prompt);
    setMaxIterations(draft.max_iterations);
    setTools(draft.tools);
  };

  const generate = async () => {
    if (!session) return setError("Start a conversation first — drafting needs a model.");
    setGenerating(true);
    setError(null);
    try {
      apply(await api.generateAgent(ask, session.provider_id, session.model));
    } catch (e) {
      setError(String(e));
    } finally {
      setGenerating(false);
    }
  };

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      await api.saveAgent(
        { name, description, prompt, tools, max_iterations: maxIterations },
        target,
      );
      await onSaved();
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  };

  const toggleTool = (tool: string) =>
    setTools((current) =>
      current === null
        ? [tool]
        : current.includes(tool)
          ? current.filter((t) => t !== tool)
          : [...current, tool],
    );

  const overLimit = description.length > DESCRIPTION_LIMIT;
  const complete = name.trim() && description.trim() && prompt.trim();

  return (
    <Modal onClose={onClose} className="scrim modal-scrim">
      <div className="modal agent-editor" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-head">
          <h2>New agent</h2>
          <span className={`tag ${target === "project" ? "project" : ""}`}>
            {target === "project" ? "project" : "all projects"}
          </span>
          <button className="drawer-close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>

        <section className="agent-generate">
          <span className="micro">Describe what it should do</span>
          <textarea
            rows={2}
            value={ask}
            placeholder="e.g. reviews a diff for correctness bugs, read-only, use after a change is written"
            aria-label="Describe what it should do"
            onChange={(e) => setAsk(e.target.value)}
          />
          <div className="agent-generate-row">
            <span className="hint">
              Taurus drafts the name, description, tools and system prompt
              below — every field stays editable.
            </span>
            <button
              className="primary"
              disabled={generating || !ask.trim()}
              onClick={generate}
            >
              {generating ? "Drafting…" : "Generate"}
            </button>
          </div>
        </section>

        <div className="agent-fields">
          <div className="field-row">
            <label className="field">
              <span className="micro">Name</span>
              <input
                className="mono"
                value={name}
                placeholder="code-reviewer"
                onChange={(e) => setName(e.target.value)}
              />
              <span className="hint mono">
                kebab-case · saved as {name.trim() || "name"}.md
              </span>
            </label>
            <label className="field narrow">
              <span className="micro">Max iterations</span>
              <input
                className="mono"
                type="number"
                min={1}
                max={MAX_ITERATIONS_LIMIT}
                value={maxIterations}
                onChange={(e) => setMaxIterations(Number(e.target.value))}
              />
              <span className="hint mono">1–{MAX_ITERATIONS_LIMIT}</span>
            </label>
          </div>

          <label className="field">
            <span className="field-label">
              <span className="micro">Description</span>
              <span className={`hint mono${overLimit ? " over" : ""}`}>
                {description.length} / {DESCRIPTION_LIMIT}
              </span>
            </span>
            <textarea
              rows={2}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
            <span className="hint">
              The only text the parent model sees when deciding whether to
              delegate here — keep it specific.
            </span>
          </label>

          <div className="field">
            <span className="field-label">
              <span className="micro">Tools</span>
              <label className="inherit">
                <input
                  type="checkbox"
                  checked={tools === null}
                  onChange={(e) => setTools(e.target.checked ? null : [])}
                />
                Inherit caller's tools
              </label>
            </span>
            {/* Every tool this session has, not a compiled-in list: an agent
                scoped to something that is not here is refused on save, and
                finding that out from the picker beats finding it from an
                error. */}
            <div className="tool-chips">
              {available.map((tool) => (
                <button
                  key={tool}
                  className={`tool-chip pick${tools?.includes(tool) ? " on" : ""}`}
                  disabled={tools === null}
                  onClick={() => toggleTool(tool)}
                >
                  {tools?.includes(tool) ? "✓ " : ""}
                  {tool}
                </button>
              ))}
            </div>
          </div>

          <label className="field">
            <span className="field-label">
              <span className="micro">System prompt</span>
              <span className="hint mono">markdown · the .md body</span>
            </span>
            <textarea
              className="mono"
              rows={6}
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
            />
          </label>

          {error && <p className="settings-problem">{error}</p>}
        </div>

        <footer className="agent-editor-foot">
          <span className="micro">Save to</span>
          <button
            className={`pill${target === "project" ? " on" : ""}`}
            onClick={() => setTarget("project")}
          >
            this project
          </button>
          <button
            className={`pill${target === "user" ? " on" : ""}`}
            onClick={() => setTarget("user")}
          >
            all projects
          </button>
          <div className="spacer" />
          <button className="quiet" onClick={onClose}>
            Cancel
          </button>
          <button
            className="primary"
            disabled={saving || !complete || overLimit}
            onClick={save}
          >
            {saving ? "Saving…" : "Save agent"}
          </button>
        </footer>
      </div>
    </Modal>
  );
}
