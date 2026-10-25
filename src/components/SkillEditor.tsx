import { useEffect, useRef, useState } from "react";

import * as api from "../lib/api";
import type { SkillProposal } from "../lib/api";
import { useStore } from "../state/store";
import { DrawerHead } from "./Drawer";
import { Modal } from "./Modal";
import { Problem } from "./Problem";
import { label, SaveTargetPills, split, usePluginTargets, type SaveTo } from "./SaveTarget";

/** The trigger line's ceiling, as the loader enforces it (`TRIGGER_MAX`). */
export const TRIGGER_LIMIT = 200;

/** The fields this editor writes. Everything else a skill can carry is the file's. */
export type SkillDraft = Pick<SkillProposal, "name" | "description" | "when_to_use" | "body">;

/**
 * Writing a skill without already knowing the format.
 *
 * The sibling of `AgentEditor`, and it keeps the same bargain: the file is the
 * source of truth, this writes it through the same validation and the same
 * writer an approved proposal takes, and every constraint the loader enforces
 * is visible in the form rather than found by having a save refused.
 *
 * It makes new skills, and fills in a proposal someone wants to change before
 * approving it. It does not edit a skill already on disk: that is opening the
 * file, because a skill's frontmatter carries keys this form doesn't know, and
 * rebuilding the file from four fields would delete them.
 *
 * No scripts or reference files. Those are files beside `SKILL.md`, added
 * after it exists, and the hint below says so rather than leaving someone to
 * look for a field that isn't there.
 */
export function SkillEditor({
  initial,
  title = "New skill",
  offerPlugins = true,
  onSubmit,
  onClose,
  onSaved,
}: {
  /** Fields to start from — a proposal being changed before it's approved. */
  initial?: SkillDraft;
  title?: string;
  /** Whether plugins you made are offered as a place to save. */
  offerPlugins?: boolean;
  /**
   * What Save does. Creates a new skill unless given, which is how a
   * proposal's card hands its edits back through its own approval.
   */
  onSubmit?: (draft: SkillDraft, target: "project" | "user", plugin?: string) => Promise<unknown>;
  onClose: () => void;
  onSaved: () => void | Promise<void>;
}) {
  const session = useStore((s) => s.session);

  const [ask, setAsk] = useState("");
  const [generating, setGenerating] = useState(false);

  const [name, setName] = useState(initial?.name ?? "");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [trigger, setTrigger] = useState(initial?.when_to_use ?? "");
  const [body, setBody] = useState(initial?.body ?? "");
  const [to, setTo] = useState<SaveTo>("project");
  const plugins = usePluginTargets(offerPlugins);

  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const drafting = useRef(false);
  const stopped = useRef(false);

  // Closing the editor ends a draft nobody is waiting for — on a local model
  // it is minutes of the machine's time. The same rule `AgentEditor` keeps.
  useEffect(
    () => () => {
      if (drafting.current) void api.stopSkillDraft().catch(() => {});
    },
    [],
  );

  const generate = async () => {
    if (!session) return setError("Start a conversation first — drafting needs a model.");
    setGenerating(true);
    drafting.current = true;
    stopped.current = false;
    setError(null);
    try {
      const draft = await api.generateSkill(ask, session.provider_id, session.model);
      setName(draft.name);
      setDescription(draft.description);
      setTrigger(draft.when_to_use);
      setBody(draft.body);
    } catch (e) {
      if (!stopped.current) setError(String(e));
    } finally {
      drafting.current = false;
      setGenerating(false);
    }
  };

  const stopDraft = () => {
    stopped.current = true;
    api.stopSkillDraft().catch((e) => setError(String(e)));
  };

  const save = async () => {
    setSaving(true);
    setError(null);
    const draft = { name: name.trim(), description, when_to_use: trigger, body };
    const { target, plugin } = split(to);
    try {
      await (onSubmit
        ? onSubmit(draft, target, plugin)
        : api.createSkill(draft, target, plugin));
      await onSaved();
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  };

  const overLimit = trigger.length > TRIGGER_LIMIT;
  const complete = name.trim() && description.trim() && trigger.trim() && body.trim();

  return (
    <Modal onClose={onClose} className="scrim modal-scrim">
      <div className="modal agent-editor skill-editor" onClick={(e) => e.stopPropagation()}>
        <DrawerHead title={title} onClose={onClose}>
          <span className={`tag ${to === "project" ? "project" : ""}`}>{label(to)}</span>
        </DrawerHead>

        {!initial && (
          <section className="agent-generate">
            <span className="micro">Describe what it should do</span>
            <textarea
              rows={2}
              value={ask}
              placeholder="e.g. cut a release: run the tests, bump the version, tag and push"
              aria-label="Describe what it should do"
              onChange={(e) => setAsk(e.target.value)}
            />
            <div className="agent-generate-row">
              <span className="hint">
                Taurus drafts the name, trigger line and steps below — every
                field stays editable.
              </span>
              <button
                className="primary"
                disabled={generating || !ask.trim()}
                onClick={generate}
              >
                {generating ? "Drafting…" : "Generate"}
              </button>
              {generating && (
                <button className="quiet" onClick={stopDraft}>
                  Stop drafting
                </button>
              )}
            </div>
          </section>
        )}

        <div className="agent-fields">
          <label className="field">
            <span className="micro">Name</span>
            <input
              className="font-mono"
              value={name}
              placeholder="cut-release"
              onChange={(e) => setName(e.target.value)}
            />
            <span className="hint font-mono">
              kebab-case · saved as {name.trim() || "name"}/SKILL.md · run it as /
              {name.trim() || "name"}
            </span>
          </label>

          <label className="field">
            <span className="micro">Description</span>
            <input value={description} onChange={(e) => setDescription(e.target.value)} />
            <span className="hint">What it does, in a sentence. Other tools show this.</span>
          </label>

          <label className="field">
            <span className="field-label">
              <span className="micro">When to use it</span>
              <span className={`hint font-mono${overLimit ? " over" : ""}`}>
                {trigger.length} / {TRIGGER_LIMIT}
              </span>
            </span>
            <textarea rows={2} value={trigger} onChange={(e) => setTrigger(e.target.value)} />
            <span className="hint">
              The only line the model reads when deciding whether to open it —
              describe the situation, not the skill.
            </span>
          </label>

          <label className="field">
            <span className="field-label">
              <span className="micro">Procedure</span>
              <span className="hint font-mono">markdown · the SKILL.md body</span>
            </span>
            <textarea
              className="font-mono"
              rows={8}
              value={body}
              onChange={(e) => setBody(e.target.value)}
            />
            <span className="hint">
              $ARGUMENTS is replaced by whatever follows /{name.trim() || "name"}.
              Scripts and references go beside SKILL.md once it's saved — open
              the skill from the drawer to add them.
            </span>
          </label>

          {error && <Problem>{error}</Problem>}
        </div>

        <footer className="agent-editor-foot">
          <SaveTargetPills value={to} onChange={setTo} plugins={plugins} />
          <div className="spacer" />
          <button className="quiet" onClick={onClose}>
            Cancel
          </button>
          <button
            className="primary"
            disabled={saving || !complete || overLimit}
            onClick={save}
          >
            {saving ? "Saving…" : "Save skill"}
          </button>
        </footer>
      </div>
    </Modal>
  );
}
