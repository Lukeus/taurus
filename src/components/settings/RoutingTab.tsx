// The Routing tab: which model each kind of job runs on, when it shouldn't be
// the conversation's.

import { useEffect, useState } from "react";

import type { AgentSummary, ModelRoute, ModelRoutes } from "../../lib/api";
import * as api from "../../lib/api";
import { useStore } from "../../state/store";
import { Problem, Problems } from "../Problem";

/** A route as the form holds it: both fields always present, empty meaning unset. */
const EMPTY: ModelRoute = { provider: "", model: "" };

/**
 * One job's provider and model.
 *
 * Commits when the provider changes or the model field is left, like the
 * fields on Terminal: a model id typed a letter at a time would otherwise be
 * saved, and checked, at every letter.
 */
function RouteRow({
  label,
  hint,
  route,
  providers,
  models,
  onWantModels,
  onCommit,
}: {
  label: string;
  hint: string;
  route: ModelRoute | undefined;
  providers: string[];
  /** What each provider lists, once asked. */
  models: Record<string, string[]>;
  onWantModels: (provider: string) => void;
  onCommit: (next: ModelRoute) => void;
}) {
  const saved = route ?? EMPTY;
  const [provider, setProvider] = useState(saved.provider);
  const [model, setModel] = useState(saved.model);
  // The store is the authority, as on Terminal.
  useEffect(() => setProvider(saved.provider), [saved.provider]);
  useEffect(() => setModel(saved.model), [saved.model]);

  const commit = (next: ModelRoute) => {
    if (next.provider === saved.provider && next.model.trim() === saved.model) return;
    onCommit(next);
  };
  const id = `route-${label.toLowerCase().replace(/\W+/g, "-")}`;
  const listed = provider ? models[provider] : undefined;

  return (
    <div className="settings-field">
      <span className="micro">{label}</span>
      <div className="grid grid-cols-[minmax(0,2fr)_minmax(0,3fr)] gap-2">
        <select
          aria-label={`${label} provider`}
          value={provider}
          onChange={(e) => {
            const next = e.target.value;
            setProvider(next);
            if (next) onWantModels(next);
            // Only once there's a model: a provider alone isn't a route yet.
            if (model.trim()) commit({ provider: next, model });
          }}
        >
          <option value="">Conversation's provider</option>
          {providers.map((id) => (
            <option key={id} value={id}>
              {id}
            </option>
          ))}
          {/* A route naming a provider since removed keeps showing what it
              names, so the problem below has something on screen to match. */}
          {provider && !providers.includes(provider) && (
            <option value={provider}>{provider} (not configured)</option>
          )}
        </select>
        <input
          aria-label={`${label} model`}
          className="font-mono"
          spellCheck={false}
          list={listed?.length ? id : undefined}
          placeholder="the conversation's model"
          value={model}
          onFocus={() => provider && onWantModels(provider)}
          onChange={(e) => setModel(e.target.value)}
          onBlur={() => commit({ provider, model })}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
          }}
        />
        {listed?.length ? (
          <datalist id={id}>
            {listed.map((m) => (
              <option key={m} value={m} />
            ))}
          </datalist>
        ) : null}
      </div>
      <span className="hint">{hint}</span>
    </div>
  );
}

/** What an agent row says, given what its own file names. */
function agentHint(agent: AgentSummary): string {
  if (agent.model) {
    const where = agent.provider ? ` on ${agent.provider}` : "";
    return `Its file names ${agent.model}${where}, which wins while it can run. This is used only if it can't.`;
  }
  return agent.description;
}

export function RoutingTab() {
  const status = useStore((s) => s.status);
  const refresh = useStore((s) => s.refresh);
  const saved = status?.settings.routes;
  const routes: ModelRoutes = saved ?? { agents: {} };
  const providers = (status?.providers ?? []).map((p) => p.id);
  const problems = (status?.problems ?? []).filter((p) => p.source === "routes");

  const [agents, setAgents] = useState<AgentSummary[]>([]);
  const [models, setModels] = useState<Record<string, string[]>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .listAgents()
      .then(setAgents)
      .catch((e) => setError(`Couldn't list the sub-agents: ${e}`));
    // Again after a save, which can change which route an agent is on.
  }, [saved]);

  // Asked once per provider, and only when a row wants it: listing models is a
  // round trip to the backend, and a provider that's down shouldn't hold up
  // the tab. A failed listing just leaves the field free text.
  const wantModels = (provider: string) => {
    if (models[provider]) return;
    setModels((m) => ({ ...m, [provider]: [] }));
    api
      .listModels(provider)
      .then((list) => setModels((m) => ({ ...m, [provider]: list.map((x) => x.id) })))
      .catch(() => {});
  };

  const save = async (next: ModelRoutes) => {
    setError(null);
    try {
      await api.setRoutes(next);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const row = (
    label: string,
    hint: string,
    route: ModelRoute | undefined,
    apply: (next: ModelRoute) => ModelRoutes,
  ) => (
    <RouteRow
      key={label}
      label={label}
      hint={hint}
      route={route}
      providers={providers}
      models={models}
      onWantModels={wantModels}
      onCommit={(next) => void save(apply(next))}
    />
  );

  return (
    <>
      <p className="drawer-intro">
        Send some jobs to a different model: a cheap or local one for small
        work, a stronger one where it counts. A job with no route runs on the
        conversation's model. Saved to <code>~/.taurus/settings.json</code>.
        A project's own settings can't set these, since they decide where your
        code is sent and who pays for it.
      </p>

      {problems.length > 0 && <Problems problems={problems.map((p) => p.message)} />}
      {error && <Problem>{error}</Problem>}

      <section className="section">
        <span className="micro">Jobs</span>
        {row(
          "Drafts",
          "Writing a first draft of an agent or a skill from its editor's Generate button.",
          routes.utility,
          (utility) => ({ ...routes, utility }),
        )}
        {row(
          "Reviews",
          "Reviewing a turn's changes. A different strong model is often the better choice here, so the work isn't marked by the model that did it.",
          routes.review,
          (review) => ({ ...routes, review }),
        )}
        {row(
          "Sub-agents",
          "Every sub-agent that doesn't name its own model or have a route below.",
          routes.delegate,
          (delegate) => ({ ...routes, delegate }),
        )}
        <p className="hint">
          Compaction always runs on the conversation's model. It reads the
          conversation's cached history, and on a hosted model a cached read
          costs a tenth of a fresh one, so a cheaper model reading it uncached
          usually costs more as well as summarizing worse.
        </p>
      </section>

      {agents.length > 0 && (
        <section className="section">
          <span className="micro">One sub-agent</span>
          {agents.map((agent) =>
            row(agent.name, agentHint(agent), routes.agents[agent.name], (route) => ({
              ...routes,
              agents: { ...routes.agents, [agent.name]: route },
            })),
          )}
        </section>
      )}
    </>
  );
}
