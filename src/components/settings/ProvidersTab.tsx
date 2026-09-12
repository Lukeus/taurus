// The Models tab's parts: one provider's form, its model list, and the checks run before a save.

import { useState } from "react";
import type {
  KeyStatus,
  ModelEntry,
  ProviderConfig,
  ProviderKind,
} from "../../lib/api";
import * as api from "../../lib/api";
import { ApiKeyField } from "./ApiKeyField";
import { Field, blank, same } from "./Field";

export const KINDS: { value: ProviderKind; label: string }[] = [
  { value: "ollama", label: "Ollama" },
  { value: "open_ai_compatible", label: "OpenAI-compatible" },
  { value: "anthropic", label: "Anthropic" },
  { value: "gemini", label: "Google Gemini" },
];

/**
 * Which fields a kind actually reads.
 *
 * Written out per kind rather than inferred from "is it Ollama", because the
 * answer stopped being binary: an Anthropic provider takes a key but has no
 * header to choose, probes its own context window but accepts a fallback, and
 * has a thinking setting nothing else does. A field on screen that the adapter
 * ignores is a setting someone will change and then wonder about.
 */
export const FIELDS: Record<
  ProviderKind,
  {
    key: boolean;
    models: boolean;
    /**
     * Route shape: where the key rides, and what path the API sits under.
     *
     * Null where the route is genuinely fixed and there is nothing to choose.
     * Otherwise the two defaults and what to say about them — per kind, because
     * they differ, and a hint written for one adapter shown over another's
     * field is worse than no hint at all.
     */
    routing: {
      header: string;
      headerHint: string;
      prefix: string;
      prefixHint: string;
    } | null;
    /** Tool support has to be declared because it cannot be probed. */
    declareTools: boolean;
    /**
     * Whether the models read images, for the one kind where it is in doubt.
     * Ollama probes it, and Anthropic and Gemini take images on everything
     * they serve — only an OpenAI-compatible endpoint might be fronting
     * text-only weights and have no way to say so.
     */
    declareVision: boolean;
    /** A context length the backend cannot report. */
    declareContext: boolean;
    /** A context length used only when the backend will not answer. */
    contextFallback: boolean;
    /**
     * A ceiling on a context length the backend reports perfectly well.
     *
     * Ollama's case, and the reason this is a third kind rather than one of
     * the two above: a local model reports the window it was *trained* for,
     * and the machine running it usually cannot serve that at any speed worth
     * waiting for. Left blank it is capped at a default rather than unbounded.
     */
    contextCap: boolean;
    thinking: boolean;
  }
> = {
  // Probes everything about itself, locally, with no credential. The one
  // number it reports that is not the whole answer is the context window: it
  // says what the model was trained for, not what this machine can serve.
  ollama: {
    key: false,
    models: false,
    routing: null,
    declareTools: false,
    declareVision: false,
    declareContext: false,
    contextFallback: false,
    contextCap: true,
    thinking: false,
  },
  // Reports nothing about itself, so all of it has to be declared.
  open_ai_compatible: {
    key: true,
    models: true,
    routing: {
      header: "Authorization: Bearer",
      headerHint:
        "Leave blank for Authorization: Bearer, which OpenAI and everything imitating it read. Azure OpenAI reads api-key; an Azure APIM gateway reads Ocp-Apim-Subscription-Key. Both bare, with no scheme in front.",
      prefix: "/v1",
      prefixHint:
        "Defaults to /v1. Azure OpenAI behind APIM usually needs /openai/v1; OpenVINO Model Server before 2026.3 needs /v3.",
    },
    declareTools: true,
    declareVision: true,
    declareContext: true,
    contextCap: false,
    contextFallback: false,
    thinking: false,
  },
  // Reports its own window and capabilities per model. The route is fixed at
  // the API itself — `x-api-key`, under `/v1` — but not through a gateway in
  // front of it, which is why these two are offered rather than assumed.
  anthropic: {
    key: true,
    models: true,
    routing: {
      header: "x-api-key",
      headerHint:
        "Leave blank for x-api-key, which this API reads directly — never Authorization: Bearer. Behind a gateway the key is the gateway's: an Azure APIM route reads Ocp-Apim-Subscription-Key, and supplies the Anthropic key itself.",
      prefix: "/v1",
      prefixHint:
        "Defaults to /v1, which is right for api.anthropic.com. A gateway publishes the API under a path of its own — an APIM route mapping straight onto /messages wants this left empty.",
    },
    declareTools: false,
    declareVision: false,
    declareContext: false,
    contextCap: false,
    contextFallback: true,
    thinking: true,
  },
  gemini: {
    key: true,
    models: true,
    routing: null,
    declareTools: false,
    declareVision: false,
    declareContext: false,
    contextCap: false,
    contextFallback: true,
    thinking: false,
  },
};

/**
 * A provider as the Models tab edits it, with an identity of its own.
 *
 * Each card holds things the draft does not — an API key typed but not yet
 * stored, whether it is open — and React keeps those with whatever key the card
 * has. Keyed by position, removing the first card would hand both to the one
 * that moves up into its place, and a key typed for one provider would be a
 * press of Store away from being filed under another. The provider's own id
 * cannot be the key either: it is one of the fields being edited, and two
 * unsaved providers can share it.
 */
export type Row = { row: number; provider: ProviderConfig };

let rowsMade = 0;

export function newRow(provider: ProviderConfig): Row {
  rowsMade += 1;
  return { row: rowsMade, provider };
}

/**
 * Rows for a list read from disk, reusing `previous`'s identities when the list
 * is the one those rows were saved as — the same length, in the same order — so
 * a save does not remount every card and fold the one being worked on. A list
 * that came back a different length is not that list, and gets fresh rows
 * rather than guessed ones.
 */
export function rowsOf(list: ProviderConfig[], previous?: Row[] | null): Row[] {
  if (previous && previous.length === list.length) {
    return list.map((provider, i) => ({ row: previous[i].row, provider }));
  }
  return list.map((provider) => newRow(provider));
}

/**
 * One provider, folded to a single row until it is being worked on.
 *
 * A configured provider is a dozen fields, and a machine with four of them —
 * a local Ollama, a gateway, and two hosted APIs — turned this tab into a page
 * of forms to scroll past to reach the one being changed. Folded, the tab is a
 * list of what is configured, which is what it is read for most of the time.
 *
 * The id and the kind stay on the header row rather than moving into the body:
 * they are what the row is scanned for, and a summary that could not be edited
 * where it is read would mean opening a card to rename it.
 */
export function ProviderForm({
  provider,
  problems,
  overriddenBy,
  keyStatus,
  keychainAvailable,
  keysUnreadable,
  onKeyChanged,
  onChange,
  onRemove,
}: {
  provider: ProviderConfig;
  /** What is wrong with this one, for the mark on a folded card. */
  problems: string[];
  overriddenBy: string[];
  keyStatus: KeyStatus | undefined;
  keychainAvailable: boolean;
  /** Why the stored keys could not be listed, when they could not. */
  keysUnreadable: string | null;
  onKeyChanged: () => void;
  onChange: (patch: Partial<ProviderConfig>) => void;
  onRemove: () => void;
}) {
  // What this kind reads, rather than "is it the OpenAI one". Showing a field
  // the adapter ignores invites a setting someone changes and then wonders
  // about; hiding one it needs makes the provider unusable from this screen.
  const fields = FIELDS[provider.kind] ?? FIELDS.open_ai_compatible;
  const compatible = fields.models;

  /*
   * Open when there is something wrong with it, which covers both cases worth
   * opening for: a provider just added, which has no base URL yet, and one that
   * is stopping the tab from saving.
   *
   * Read once rather than followed, so typing the missing URL does not fold the
   * card shut under the cursor. After that it is the user's to decide, and a
   * card they folded stays folded — with a mark on it, so a save refused over
   * something out of sight still says where to look.
   */
  const [open, setOpen] = useState(() => problems.length > 0);

  return (
    <div
      className="card settings-provider p-3 flex flex-col gap-3 not-data-open:py-2"
      data-open={open || undefined}
    >
      <div className="card-row">
        <button
          type="button"
          className="settings-provider-fold flex-none w-5.5 self-stretch p-0 border-0 bg-transparent text-12 leading-none text-faint hover:text-ink focus-visible:rounded-sm focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-1"
          aria-expanded={open}
          aria-label={`${open ? "Collapse" : "Expand"} ${provider.id || "this provider"}`}
          onClick={() => setOpen((was) => !was)}
        >
          {/* The same two glyphs the plan panel and the run header disclose
              with, so one gesture is learned once. Decorative beside
              `aria-expanded`, which is the same fact where a screen reader
              looks for it. */}
          <span aria-hidden="true">{open ? "⌄" : "›"}</span>
        </button>
        <input
          className="settings-id flex-1 min-w-0 font-mono"
          value={provider.id}
          aria-label="Provider id"
          placeholder="id"
          onChange={(e) => onChange({ id: e.target.value })}
        />
        <select
          value={provider.kind}
          aria-label="Provider kind"
          onChange={(e) => onChange({ kind: e.target.value as ProviderKind })}
        >
          {KINDS.map((kind) => (
            <option key={kind.value} value={kind.value}>
              {kind.label}
            </option>
          ))}
        </select>
        {!open && problems.length > 0 && (
          <span
            className="dot error"
            role="img"
            aria-label={problems.join(" ")}
            data-tip={problems.join(" ")}
          />
        )}
        <button className="danger" onClick={onRemove}>
          Remove
        </button>
      </div>

      {open && (
        <>
      <Field label="Base URL">
        <input
          value={provider.base_url}
          placeholder="http://localhost:11434"
          onChange={(e) => onChange({ base_url: e.target.value })}
        />
      </Field>

      {compatible && (
        <ModelList
          models={provider.models}
          onChange={(models) => onChange({ models })}
        />
      )}

      <Field
        label="Default model"
        hint={
          compatible
            ? "Which one a new conversation starts on. Optional — the first model above is used otherwise."
            : "Which one a new conversation starts on. Optional."
        }
      >
        <input
          value={provider.default_model ?? ""}
          placeholder="optional"
          onChange={(e) => onChange({ default_model: blank(e.target.value) })}
        />
      </Field>

      {fields.key && (
        <>
          <ApiKeyField
            status={keyStatus}
            available={keychainAvailable}
            onStore={(key) => api.setProviderKey(provider.id, key)}
            onClear={() => api.clearProviderKey(provider.id)}
            onChanged={onKeyChanged}
            unreadable={keysUnreadable}
            unsavedHint="Save this provider before storing a key for it."
          />

          <Field
            label="API key variable"
            hint="Optional. Names an environment variable that overrides the stored key — useful for CI, and the only option on machines with no keychain."
          >
            <input
              value={provider.api_key_env ?? ""}
              placeholder="e.g. OPENAI_API_KEY"
              onChange={(e) => onChange({ api_key_env: blank(e.target.value) })}
            />
          </Field>

          {fields.routing && (
            <>
              <Field label="API key header" hint={fields.routing.headerHint}>
                <input
                  value={provider.api_key_header ?? ""}
                  placeholder={fields.routing.header}
                  onChange={(e) =>
                    onChange({ api_key_header: blank(e.target.value) })
                  }
                />
              </Field>

              <Field label="API prefix" hint={fields.routing.prefixHint}>
                <input
                  value={provider.api_prefix ?? ""}
                  placeholder={fields.routing.prefix}
                  onChange={(e) => onChange({ api_prefix: blank(e.target.value) })}
                />
              </Field>
            </>
          )}

          {fields.declareTools && (
            <Field
              label="Tool calling"
              hint="These cannot be probed over the OpenAI API, so they have to be declared."
            >
              <select
                value={triState(provider.native_tools)}
                onChange={(e) =>
                  onChange({ native_tools: fromTriState(e.target.value) })
                }
              >
                <option value="auto">Assume supported</option>
                <option value="yes">Native</option>
                <option value="no">Prompted — model has no tool support</option>
              </select>
            </Field>
          )}

          {fields.declareVision && (
            <Field
              label="Images"
              hint="Turn this off for an endpoint serving text-only weights, so an attached screenshot is refused here with a reason rather than a round trip away."
            >
              <select
                value={triState(provider.vision)}
                onChange={(e) => onChange({ vision: fromTriState(e.target.value) })}
              >
                <option value="auto">Assume readable</option>
                <option value="yes">Model reads images</option>
                <option value="no">Text only — refuse attachments</option>
              </select>
            </Field>
          )}

          {fields.thinking && (
            <Field
              label="Thinking"
              hint="Leave on the model's default unless you have a reason: it is the only setting valid on every Claude model, and the wrong one is rejected rather than ignored."
            >
              <select
                value={provider.thinking ?? ""}
                onChange={(e) => onChange({ thinking: blank(e.target.value) })}
              >
                <option value="">Model's default</option>
                <option value="adaptive">Adaptive — the model decides</option>
                <option value="disabled">Off</option>
              </select>
            </Field>
          )}

          {(fields.declareContext ||
            fields.contextFallback ||
            fields.contextCap) && (
            <Field
              label="Context length"
              hint={
                fields.contextCap
                  ? "A ceiling, not a request. A local model reports the window it was trained for, which is often far more than this machine can serve — allocating all of it can make a model many times slower. Blank means 32768, or the model's own window when that is smaller."
                  : fields.contextFallback
                    ? "Only used if the backend will not report its own window. Drives when history is compacted."
                    : "Drives when history is compacted. 8192 for OpenVINO on NPU."
              }
            >
              <input
                inputMode="numeric"
                value={provider.context_length ?? ""}
                placeholder={
                  fields.contextCap
                    ? "32768"
                    : fields.contextFallback
                      ? "probed"
                      : "128000"
                }
                onChange={(e) =>
                  onChange({ context_length: parseContextLength(e.target.value) })
                }
              />
            </Field>
          )}
        </>
      )}

      {overriddenBy.length > 0 && (
        <p className="settings-note m-0 text-12 leading-[1.5] text-warn">
          This workspace overrides {listSentence(overriddenBy)} in its own{" "}
          <code>.taurus/providers.json</code>. Changes here apply everywhere
          else; the override still wins in this project.
        </p>
      )}
        </>
      )}
    </div>
  );
}

/**
 * The models this endpoint serves.
 *
 * Only shown for OpenAI-compatible providers. Ollama reports its own inventory
 * *and* its own per-model capabilities, so a list here would be a second answer
 * to a question already answered correctly.
 *
 * Empty is the normal state and means "ask the endpoint". Naming anything
 * replaces that listing outright, which is what makes this usable two ways: a
 * gateway with no `/v1/models` route can finally offer more than one model, and
 * one that lists four hundred can be cut to the three there is quota for.
 *
 * The per-model overrides are here rather than only on the provider because a
 * gateway routinely fronts models that share neither a context window nor tool
 * support, and the wire format reports neither.
 */
export function ModelList({
  models,
  onChange,
}: {
  models: ModelEntry[];
  onChange: (models: ModelEntry[]) => void;
}) {
  const patch = (index: number, fields: Partial<ModelEntry>) =>
    onChange(models.map((m, i) => (i === index ? { ...m, ...fields } : m)));

  return (
    <div className="settings-field">
      <div className="section-head">
        <span className="micro">Models</span>
        <button
          className="link"
          onClick={() => onChange([...models, { id: "" }])}
        >
          Add a model
        </button>
      </div>

      <ul className="model-rows list-none m-0 p-0 flex flex-col gap-2.5">
        {models.map((model, i) => (
          // Keyed by position because nothing else is stable: the id is the
          // field being typed into, and is empty on a row just added.
          <li
            key={i}
            className="model-row grid grid-cols-[1fr_auto] gap-x-2 gap-y-1.5 items-center [&_input]:min-w-0 [&_input]:w-full [&_select]:min-w-0 [&_select]:w-full"
          >
            <input
              className="font-mono"
              aria-label={`Model ${i + 1} id`}
              value={model.id}
              placeholder="gpt-4o"
              onChange={(e) => patch(i, { id: e.target.value })}
            />
            <button
              className="quiet model-remove py-1.5 px-2 text-faint"
              aria-label={`Remove ${model.id || `model ${i + 1}`}`}
              onClick={() => onChange(models.filter((_, n) => n !== i))}
            >
              ✕
            </button>
            {/*
                Two rows rather than one, because one never fitted.

                The three controls used to sit in a row with the selects capped
                at 46% each — which is 92% of a 369px block before the gaps, so
                the context length was left with thirteen pixels and could not
                be typed into at all. It is a box for a six-digit number.

                So the number takes a line of its own and the two switches share
                the one under it. Each select comes out at 180px, wider than the
                170px the cap allowed, so nothing that fitted before stops
                fitting; what changes is that the field beside them exists.
            */}
            <div className="model-caps col-span-full grid grid-cols-2 gap-x-2 gap-y-1.5 pl-3 border-l border-rule [&_input]:col-span-full">
              <input
                inputMode="numeric"
                aria-label={`Context length for ${model.id || `model ${i + 1}`}`}
                value={model.context_length ?? ""}
                placeholder="context length"
                onChange={(e) =>
                  patch(i, { context_length: parseContextLength(e.target.value) })
                }
              />
              <select
                aria-label={`Tool calling for ${model.id || `model ${i + 1}`}`}
                value={triState(model.native_tools ?? null)}
                onChange={(e) =>
                  patch(i, { native_tools: fromTriState(e.target.value) })
                }
              >
                <option value="auto">Tools: as below</option>
                <option value="yes">Tools: native</option>
                <option value="no">Tools: prompted</option>
              </select>
              <select
                aria-label={`Images for ${model.id || `model ${i + 1}`}`}
                value={triState(model.vision ?? null)}
                onChange={(e) => patch(i, { vision: fromTriState(e.target.value) })}
              >
                <option value="auto">Images: as below</option>
                <option value="yes">Images: readable</option>
                <option value="no">Images: text only</option>
              </select>
            </div>
          </li>
        ))}
      </ul>

      <span className="hint">
        {models.length === 0
          ? "None named, so Taurus asks this endpoint what it serves. Add one to decide the menu yourself — the only option on a gateway with no /v1/models route."
          : "These replace whatever the endpoint would list. Left blank, a model inherits the context length and tool calling set below."}
      </span>
    </div>
  );
}

/**
 * Keeps a half-typed number from clearing a configured one.
 *
 * Returning `null` for unparseable input would delete the value the moment
 * someone selected it to retype, so only a genuinely empty box unsets it.
 */
export function parseContextLength(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const parsed = Number.parseInt(trimmed, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

export function triState(value: boolean | null): string {
  if (value === null) return "auto";
  return value ? "yes" : "no";
}

export function fromTriState(value: string): boolean | null {
  if (value === "auto") return null;
  return value === "yes";
}

/**
 * Which fields this workspace overrides for a given global provider.
 *
 * Compares the entry as stored globally against the effective one the host
 * resolved. Anything that differs came from the workspace layer, and the user
 * needs to know that editing here will not change what this project uses.
 */
export function overrideOf(
  global: ProviderConfig,
  effective: ProviderConfig[],
): string[] {
  const match = effective.find((p) => p.id === global.id);
  if (!match) return [];
  const fields: (keyof ProviderConfig)[] = [
    "kind",
    "base_url",
    "models",
    "default_model",
    "api_key_env",
    "api_key_header",
    "native_tools",
    "context_length",
    "vision",
    "api_prefix",
  ];
  return fields.filter((field) => !same(global[field], match[field]));
}

/** Duplicate ids and missing required fields, as sentences. */
/**
 * What is wrong with one provider, in the words the list at the bottom uses.
 *
 * Per provider rather than only in aggregate, because a card can be collapsed:
 * a save refused over something folded out of sight needs a mark on the card
 * that would fix it. Built from the same rules as {@link validate} rather than
 * beside them, so the mark and the message cannot come to disagree.
 *
 * A shared id implicates both cards, not only the second one. Which of two
 * identical ids is "the duplicate" is not a question this can answer, and
 * marking one of them would send someone to the wrong card half the time.
 */
export function problemsWith(
  provider: ProviderConfig,
  all: ProviderConfig[],
): string[] {
  const problems: string[] = [];
  const id = provider.id.trim();

  if (id === "") {
    problems.push("Every provider needs an id.");
  } else if (all.filter((p) => p.id.trim() === id).length > 1) {
    problems.push(`Two providers share the id "${id}".`);
  }
  if (provider.base_url.trim() === "") {
    problems.push(`"${id || "A provider"}" needs a base URL.`);
  }
  return problems;
}

export function validate(providers: ProviderConfig[]): string[] {
  // Same problem stated twice reads as two problems — and a shared id is now
  // reported by both of the cards that share it.
  return [
    ...new Set(providers.flatMap((p) => problemsWith(p, providers))),
  ];
}

/** A new entry that will not collide with an existing id. */
export function blankProvider(existing: ProviderConfig[]): ProviderConfig {
  const taken = new Set(existing.map((p) => p.id));
  let id = "new-provider";
  for (let n = 2; taken.has(id); n++) id = `new-provider-${n}`;
  return {
    id,
    kind: "open_ai_compatible",
    base_url: "",
    models: [],
    default_model: null,
    api_key_env: null,
    api_key_header: null,
    native_tools: null,
    context_length: null,
    vision: null,
    api_prefix: null,
    thinking: null,
  };
}

export function listSentence(items: string[]): string {
  const pretty = items.map((i) => i.replace(/_/g, " "));
  if (pretty.length === 1) return pretty[0];
  return `${pretty.slice(0, -1).join(", ")} and ${pretty[pretty.length - 1]}`;
}
