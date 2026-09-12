// The Search tab: the web search backend, and the code index behind `search_code`.

import { useEffect, useState } from "react";
import type { IndexProgress, SearchBackend, SearchSettings } from "../../lib/api";
import { useStore } from "../../state/store";
import { Problem } from "../Problem";
import * as api from "../../lib/api";
import { ApiKeyField } from "./ApiKeyField";
import { Field, blank } from "./Field";

/**
 * The settings a save came back with, keeping anything typed since it was sent.
 *
 * Replaced whole, the reload after a save on blur overwrote whatever had been
 * typed during the round trip. A field that differs from what was sent was
 * edited since, and that edit is newer than the answer.
 */
export function keepEdits(
  fresh: SearchSettings,
  now: SearchSettings | null,
  sent: SearchBackend[],
): SearchSettings {
  if (!now) return fresh;
  return {
    ...fresh,
    backends: fresh.backends.map((backend) => {
      const mine = now.backends.find((b) => b.id === backend.id);
      const was = sent.find((b) => b.id === backend.id);
      if (!mine || !was) return backend;
      return {
        ...backend,
        base_url: mine.base_url !== was.base_url ? mine.base_url : backend.base_url,
        api_key_env:
          mine.api_key_env !== was.api_key_env ? mine.api_key_env : backend.api_key_env,
      };
    }),
  };
}

/**
 * Web search: which backend, and the key it needs.
 *
 * Everything behind this was built and shipped some time ago — the backends,
 * the key handling, the tools — and none of it was reachable without knowing
 * `~/.taurus/search.json` existed and writing its schema by hand.
 *
 * Off is the default and stays a first-class choice, not a disabled state:
 * searching means sending the user's prompt to a third party, and that is not
 * something to slide into because a screen made it the easy option.
 */
export function SearchTab() {
  const [settings, setSettings] = useState<SearchSettings | null>(null);
  const [keychain, setKeychain] = useState(false);
  const [busy, setBusy] = useState(false);
  /** Why the last read or write did not work, when it did not. */
  const [error, setError] = useState<string | null>(null);

  // Said rather than swallowed: a read that failed in silence left the tab on
  // "Loading…" for good, which looks like a hang rather than a broken file.
  const load = () =>
    api
      .getSearchSettings()
      .then((fresh) => {
        setSettings(fresh);
        setError(null);
      })
      .catch((e) => setError(String(e)));

  useEffect(() => {
    load();
    api.keychainAvailable().then(setKeychain).catch(() => setKeychain(false));
  }, []);

  if (!settings) {
    return error ? <Problem>{error}</Problem> : <p className="drawer-intro">Loading…</p>;
  }

  const keys = new Map(settings.key_statuses);
  const selected = settings.backends.find((b) => b.id === settings.selected);

  const save = async (id: string | null, backends = settings.backends) => {
    setBusy(true);
    setError(null);
    try {
      await api.saveSearchSettings(id, backends);
      const fresh = await api.getSearchSettings();
      // Merged rather than replaced: see `keepEdits`.
      setSettings((now) => keepEdits(fresh, now, backends));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const patch = (id: string, change: Partial<SearchBackend>) =>
    settings.backends.map((b) => (b.id === id ? { ...b, ...change } : b));

  return (
    <>
      <p className="drawer-intro">
        Lets Taurus look things up on the web. Your prompt goes to whichever
        service you pick, so it stays off until you choose one.
      </p>

      {error && <Problem>{error}</Problem>}

      {settings.problems.length > 0 && (
        <section className="section">
          <span className="micro">Could not load</span>
          {settings.problems.map((problem) => (
            <Problem key={problem.message}>{problem.message}</Problem>
          ))}
        </section>
      )}

      <Field label="Search with" hint={statusHint(settings)}>
        <select
          value={settings.selected ?? ""}
          disabled={busy}
          onChange={(e) => save(e.target.value === "" ? null : e.target.value)}
        >
          <option value="">Off</option>
          {settings.backends.map((backend) => (
            <option key={backend.id} value={backend.id}>
              {backend.id}
            </option>
          ))}
        </select>
      </Field>

      {selected && (
        <div className="card-list">
          <div className="card">
            <div className="card-body">
              <Field
                label="Address"
                hint={
                  selected.kind === "searxng"
                    ? "Your SearXNG instance. There is no default — no instance is the canonical one."
                    : "Override only if you route through a proxy."
                }
              >
                <input
                  value={selected.base_url}
                  disabled={busy}
                  spellCheck={false}
                  placeholder={
                    selected.kind === "searxng" ? "http://localhost:8888" : ""
                  }
                  onChange={(e) =>
                    setSettings({
                      ...settings,
                      backends: patch(selected.id, { base_url: e.target.value }),
                    })
                  }
                  onBlur={() => save(settings.selected)}
                />
              </Field>

              {selected.needs_key && (
                <>
                  <ApiKeyField
                    status={keys.get(selected.id)}
                    available={keychain}
                    onStore={(key) => api.setSearchKey(selected.id, key)}
                    onClear={() => api.clearSearchKey(selected.id)}
                    onChanged={load}
                    unsavedHint="Save this backend before storing a key for it."
                  />

                  <Field
                    label="API key variable"
                    hint="Optional. Names an environment variable that overrides the stored key — useful for CI, and the only option on machines with no keychain."
                  >
                    <input
                      value={selected.api_key_env ?? ""}
                      disabled={busy}
                      spellCheck={false}
                      placeholder="BRAVE_API_KEY"
                      onChange={(e) =>
                        setSettings({
                          ...settings,
                          backends: patch(selected.id, {
                            api_key_env: blank(e.target.value),
                          }),
                        })
                      }
                      onBlur={() => save(settings.selected)}
                    />
                  </Field>
                </>
              )}
            </div>
          </div>
        </div>
      )}
    </>
  );
}

/**
 * An index build started from the Search tab: how far it has got, what it said
 * when it finished, and how to start one.
 *
 * A hook for `Settings` rather than state in `CodeSearch`, which unmounts with
 * its tab. The build goes on in Rust either way, and a tab switch that dropped
 * this came back to a Build button beside a build still running, with no Stop.
 */
export function useIndexBuild() {
  const [progress, setProgress] = useState<IndexProgress | null>(null);
  const [outcome, setOutcome] = useState<string | null>(null);
  const build = async () => {
    // Seeded before the first report, so the button switches to Stop on the
    // click rather than whenever the first batch lands — which on a cold
    // Ollama is several seconds of a button that looks like it did nothing.
    setProgress({ done: 0, total: 0 });
    setOutcome(null);
    try {
      setOutcome(await api.buildIndex(setProgress));
    } catch (e) {
      setOutcome(String(e));
    } finally {
      setProgress(null);
    }
  };
  return { progress, outcome, onBuild: () => void build() };
}

/**
 * Semantic search over the workspace: which model embeds it, and a way to pay
 * the first index before a turn has to.
 *
 * The build button is the whole reason this section exists. Indexing this
 * repository takes the better part of a minute, and the only way to pay that
 * used to be to be halfway through a turn when the model first reached for
 * `search_code` — which then sat on a tool call that did not return, with no
 * way to start one earlier and no way to stop it that did not also stop the
 * conversation. The model field comes with it because a button that builds an
 * index needs somewhere to say what to build it with, and `embedding_model` was
 * reachable only by hand-editing `~/.taurus/settings.json`.
 */
export function CodeSearch({
  model,
  provider,
  rerankModel,
  rerankProvider,
  progress,
  outcome,
  onBuild,
}: {
  model: string;
  provider: string;
  rerankModel: string;
  rerankProvider: string;
  /** A build in progress, held by `Settings` so a tab switch does not lose it. */
  progress: IndexProgress | null;
  outcome: string | null;
  onBuild: () => void;
}) {
  const refresh = useStore((s) => s.refresh);
  const [draft, setDraft] = useState(model);
  const [embedProviderDraft, setEmbedProviderDraft] = useState(provider);
  const [rerankDraft, setRerankDraft] = useState(rerankModel);
  const [rerankProviderDraft, setRerankProviderDraft] = useState(rerankProvider);
  // The store is the authority, as in `IterationLimit`: a refresh from
  // anywhere else — another window, a hand-edited settings file — has to win
  // over a draft seeded when this tab was opened.
  useEffect(() => setDraft(model), [model]);
  useEffect(() => setEmbedProviderDraft(provider), [provider]);
  useEffect(() => setRerankDraft(rerankModel), [rerankModel]);
  useEffect(() => setRerankProviderDraft(rerankProvider), [rerankProvider]);
  const building = progress !== null;

  // Model and provider save together, because the backend takes them together:
  // a model with no provider embeds on whichever backend the conversation is
  // using, and Anthropic has no embedding endpoint at all.
  const save = async (nextModel: string, nextProvider: string) => {
    if (
      nextModel.trim() === model.trim() &&
      nextProvider.trim() === provider.trim()
    ) {
      return;
    }
    await api.setEmbeddingModel(nextModel, nextProvider);
    await refresh();
  };

  // Both fields save together, because the backend takes them together: a
  // model with no provider reranks on whichever server the conversation is
  // using, and for the Ollama setup most people have that is a round trip that
  // fails on every search.
  const saveRerank = async (nextModel: string, nextProvider: string) => {
    if (
      nextModel.trim() === rerankModel.trim() &&
      nextProvider.trim() === rerankProvider.trim()
    ) {
      return;
    }
    await api.setRerank(nextModel, nextProvider);
    await refresh();
  };


  const pct =
    progress && progress.total > 0
      ? Math.round((progress.done / progress.total) * 100)
      : 0;

  return (
    <section className="section">
      <span className="micro">Search the codebase</span>
      <Field
        label="Embedding model"
        hint={
          model.trim()
            ? "Runs on the same server as your chat model. Leave empty to turn semantic search off."
            : "Off. Name a model your server has pulled — nomic-embed-text is the usual one — to let Taurus find code by meaning rather than by string."
        }
      >
        <input
          value={draft}
          spellCheck={false}
          placeholder="nomic-embed-text"
          disabled={building}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => save(draft, embedProviderDraft)}
        />
      </Field>

      {draft.trim() && (
        <Field
          label="Embedding provider"
          hint="Which backend serves it. Leave empty to use the one this conversation is on — name another if that backend has no embedding endpoint, which is the case for Anthropic."
        >
          <input
            value={embedProviderDraft}
            spellCheck={false}
            placeholder="the one this conversation is on"
            disabled={building}
            onChange={(e) => setEmbedProviderDraft(e.target.value)}
            onBlur={() => save(draft, embedProviderDraft)}
          />
        </Field>
      )}

      {model.trim() && (
        <>
          <div className="index-actions flex items-center gap-3">
            <button onClick={building ? api.stopIndexBuild : onBuild}>
              {building ? "Stop" : "Build index now"}
            </button>
            {building && (
              <span className="hint">
                {progress.total > 0
                  ? `${progress.done} of ${progress.total} passages`
                  : "reading the workspace…"}
              </span>
            )}
          </div>
          {building && (
            <div className="plan-bar index-bar w-full" aria-hidden="true">
              <span className="plan-bar-fill" style={{ width: `${pct}%` }} />
            </div>
          )}
          {outcome && !building && <p className="hint">{outcome}</p>}
          <p className="hint">
            A search builds this itself the first time, inside whichever turn
            reaches for it. Building it here pays the same cost where you can
            watch it and stop it.
          </p>

          <Field
            label="Reranking model"
            hint={
              rerankModel.trim()
                ? "Reads each result against your question and reorders the top thirty. Leave empty to rank by similarity alone."
                : "Optional. A reranking model reads the question and the passage together, which is more accurate than similarity — and needs a server that serves a /rerank route, which Ollama does not."
            }
          >
            <input
              value={rerankDraft}
              spellCheck={false}
              placeholder="bge-reranker-v2-m3"
              disabled={building}
              onChange={(e) => setRerankDraft(e.target.value)}
              onBlur={() => saveRerank(rerankDraft, rerankProviderDraft)}
            />
          </Field>

          {rerankDraft.trim() && (
            <Field
              label="Reranking provider"
              hint="Which backend serves it. Leave empty if the same server embeds and reranks — name one of your other providers if not."
            >
              <input
                value={rerankProviderDraft}
                spellCheck={false}
                placeholder="the one that embeds"
                disabled={building}
                onChange={(e) => setRerankProviderDraft(e.target.value)}
                onBlur={() => saveRerank(rerankDraft, rerankProviderDraft)}
              />
            </Field>
          )}
        </>
      )}
    </section>
  );
}

/**
 * Whether search is actually running, which is not the same as whether a
 * backend is picked — a selection with no key resolves to nothing and
 * registers no tools. Saying "on" then would be a lie the transcript
 * immediately contradicts.
 */
export function statusHint(settings: SearchSettings): string {
  if (settings.selected === null) return "Off. Taurus will not search the web.";
  if (settings.active) return `On, through ${settings.selected}.`;
  return `Selected, but not running yet — ${settings.selected} still needs something below.`;
}
