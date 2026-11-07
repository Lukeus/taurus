import { useEffect } from "react";
import { create } from "zustand";

import type { ApplyPlan, Kept } from "../lib/api";
import { applyDelegate, delegateRun, discardDelegate } from "../lib/api";
import { plural } from "../lib/format";
import { useStore } from "../state/store";

const IDLE: KeptState = { kind: "idle" };

/**
 * Where each kept run stands, shared by every place that draws it: the
 * delegation's card and the Agent activity panel. Held apart from either, so
 * applying from one is seen in the other rather than offered again.
 */
export const useKept = create<{
  runs: Record<string, KeptState>;
  set: (run: string, state: KeptState) => void;
}>((set) => ({
  runs: {},
  set: (run, state) => set((k) => ({ runs: { ...k.runs, [run]: state } })),
}));

/** What a click on a kept run's buttons came to. */
export type KeptState =
  | { kind: "idle" }
  | { kind: "working" }
  /** `plan` when it was applied here, and none when it was found applied. */
  | { kind: "applied"; plan?: ApplyPlan }
  | { kind: "refused"; plan: ApplyPlan }
  | { kind: "discarded" }
  | { kind: "error"; message: string };

/**
 * A delegate's work kept in its own copy of the workspace, and the two things
 * to do with it. Applying checks every file against the workspace as it is
 * now, and writes nothing unless it can write everything; see
 * `taurus_host::worktree`.
 */
export function KeptWork({ kept }: { kept: Kept }) {
  const sessionId = useStore((s) => s.session?.id ?? null);
  const busy = useStore((s) => s.busy);
  const state = useKept((k) => k.runs[kept.run] ?? IDLE);
  const setState = (next: KeptState) => useKept.getState().set(kept.run, next);

  // Asked once a card is drawn with nothing known about its run. What's held
  // here lives only as long as the window, and the run can have been applied
  // or discarded since (in Settings, from the CLI, by the model, before a
  // reload), which offered Apply again on work that was already in.
  const known = state.kind !== "idle";
  useEffect(() => {
    if (known) return;
    let live = true;
    delegateRun(kept.run)
      .then((summary) => {
        if (!live || useKept.getState().runs[kept.run]) return;
        if (summary?.state === "applied") useKept.getState().set(kept.run, { kind: "applied" });
        if (summary?.state === "discarded") {
          useKept.getState().set(kept.run, { kind: "discarded" });
        }
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [kept.run, known]);

  const act = async (what: "apply" | "discard") => {
    if (!sessionId) return;
    setState({ kind: "working" });
    try {
      if (what === "discard") {
        await discardDelegate(sessionId, kept.run);
        setState({ kind: "discarded" });
        return;
      }
      const plan = await applyDelegate(sessionId, kept.run, false);
      setState(plan.applied ? { kind: "applied", plan } : { kind: "refused", plan });
      if (plan.applied) {
        // What a turn's own write does, so a file open in the canvas or the
        // notes pane reloads, or says it has typing that would be lost, rather
        // than showing what was there before the apply.
        const paths = plan.files
          .filter((f) => f.action === "take" || f.action === "merge" || f.action === "delete")
          .map((f) => f.path);
        if (paths.length > 0) {
          useStore.setState((s) => ({ wrote: { paths, at: (s.wrote?.at ?? 0) + 1 } }));
        }
        void useStore.getState().refreshChanged();
      }
    } catch (e) {
      setState({ kind: "error", message: String(e) });
    }
  };

  const files = kept.files.length;
  const settled = state.kind === "applied" || state.kind === "discarded";
  return (
    <div className="run-row-kept" data-state={state.kind}>
      <p className="run-row-kept-head">
        {state.kind === "applied"
          ? `Applied ${plural(files, "file")} to the workspace. Rewinding this turn undoes it.`
          : state.kind === "discarded"
            ? "Discarded. Nothing from it reached the workspace."
            : `Kept in its own copy, started from ${kept.base}: ${plural(files, "file")}, not in the workspace yet.`}
      </p>
      {!settled && files > 0 && (
        <ul className="run-row-kept-files">
          {kept.files.map((file) => (
            <li key={file}>{file}</li>
          ))}
        </ul>
      )}
      {kept.outside.length > 0 && !settled && (
        <p className="run-row-kept-outside">
          Also changed outside the workspace, never applied: {kept.outside.join(", ")}
        </p>
      )}
      {state.kind === "refused" && (
        <div className="run-row-kept-refused" role="alert">
          <p>Nothing was applied, because these can't be:</p>
          <ul>
            {state.plan.files
              .filter((f) => f.action === "conflict")
              .map((f) => (
                <li key={f.path}>
                  <code>{f.path}</code>: {f.why}
                </li>
              ))}
          </ul>
        </div>
      )}
      {state.kind === "error" && (
        <p className="run-row-kept-refused" role="alert">
          {state.message}
        </p>
      )}
      {!settled && (
        <div className="run-row-links">
          <button
            className="run-row-activity"
            disabled={busy || state.kind === "working" || !sessionId}
            data-tip={busy ? "Wait for the turn to finish" : undefined}
            onClick={() => void act("apply")}
          >
            {state.kind === "working" ? "Applying…" : "Apply to workspace"}
          </button>
          <button
            className="run-row-activity"
            disabled={state.kind === "working" || !sessionId}
            onClick={() => void act("discard")}
          >
            Discard
          </button>
        </div>
      )}
    </div>
  );
}
