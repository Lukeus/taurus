import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import type { ProviderConfig, SearchSettings } from "../lib/api";
import {
  FIELDS,
  Settings,
  ModelList,
  blankProvider,
  keyHint,
  overrideOf,
  parseContextLength,
  statusHint,
  validate,
} from "./Settings";

const provider = (patch: Partial<ProviderConfig> = {}): ProviderConfig => ({
  id: "ollama",
  kind: "ollama",
  base_url: "http://localhost:11434",
  models: [],
  default_model: null,
  api_key_env: null,
  api_key_header: null,
  native_tools: null,
  context_length: null,
  vision: null,
  api_prefix: null,
  thinking: null,
  ...patch,
});

describe("what the API key field says about where the key comes from", () => {
  it("names the variable that is overriding a stored key", () => {
    // The state a user hits after storing a key that then does nothing. The
    // hint has to name the variable, or there is nothing to go and unset.
    const hint = keyHint({ kind: "overridden", variable: "OPENAI_API_KEY" });
    expect(hint).toContain("OPENAI_API_KEY");
    expect(hint).toMatch(/unset/i);
  });

  it("names the variable in use when nothing is stored", () => {
    expect(keyHint({ kind: "environment", variable: "AZURE_KEY" })).toContain(
      "AZURE_KEY",
    );
  });

  it("says a stored key is actually in use", () => {
    expect(keyHint({ kind: "keychain" })).toMatch(/in use/i);
  });

  it("promises the keychain rather than disk when nothing is set yet", () => {
    const hint = keyHint({ kind: "missing" });
    expect(hint).toMatch(/keychain/i);
    expect(hint).toMatch(/never on disk/i);
  });
});

describe("context length input", () => {
  it("treats an empty box as unset", () => {
    expect(parseContextLength("")).toBeNull();
    expect(parseContextLength("   ")).toBeNull();
  });

  it("reads a number", () => {
    expect(parseContextLength("8192")).toBe(8192);
    expect(parseContextLength(" 8192 ")).toBe(8192);
  });

  it("rejects values that would disable compaction", () => {
    // Zero or negative would make the compaction budget zero or nonsense.
    expect(parseContextLength("0")).toBeNull();
    expect(parseContextLength("-5")).toBeNull();
    expect(parseContextLength("abc")).toBeNull();
  });
});

describe("validation", () => {
  it("accepts a normal configuration", () => {
    expect(validate([provider(), provider({ id: "vllm" })])).toEqual([]);
  });

  it("catches two providers claiming the same id", () => {
    // The id is the key everything else resolves by; duplicates would make
    // which one you get depend on ordering.
    const problems = validate([provider(), provider()]);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("ollama");
  });

  it("catches a missing id and a missing base URL", () => {
    const problems = validate([provider({ id: "  ", base_url: "" })]);
    expect(problems.some((p) => p.includes("id"))).toBe(true);
    expect(problems.some((p) => p.includes("base URL"))).toBe(true);
  });

  it("states a repeated problem once", () => {
    const problems = validate([provider({ id: "" }), provider({ id: "" })]);
    expect(problems.filter((p) => p.includes("needs an id"))).toHaveLength(1);
  });
});

describe("workspace override detection", () => {
  it("says nothing when the effective value matches the global one", () => {
    const global = provider({ id: "ov", base_url: "http://a" });
    expect(overrideOf(global, [global])).toEqual([]);
  });

  it("names the fields this workspace changed", () => {
    // Editing here writes the global file, which will NOT change what this
    // project uses — the user has to be told, or the save looks broken.
    const global = provider({ id: "ov", base_url: "http://a" });
    const effective = provider({
      id: "ov",
      base_url: "http://gpu-box:11434",
      context_length: 8192,
    });
    expect(overrideOf(global, [effective]).sort()).toEqual([
      "base_url",
      "context_length",
    ]);
  });

  it("treats a provider the workspace never mentions as unoverridden", () => {
    expect(overrideOf(provider({ id: "ov" }), [provider({ id: "other" })])).toEqual(
      [],
    );
  });

  it("does not confuse null with undefined", () => {
    // ts-rs emits `string | null`, but a hand-edited file can omit the key
    // entirely; both mean "unset" and neither is an override.
    const global = provider({ id: "ov", api_prefix: null });
    const effective = { ...provider({ id: "ov" }) } as ProviderConfig;
    delete (effective as Partial<ProviderConfig>).api_prefix;
    expect(overrideOf(global, [effective])).toEqual([]);
  });
});

describe("the model list", () => {
  it("is not an override just because two equal lists are different objects", () => {
    // `!==` on arrays compares identity, so every provider with a model list
    // reported itself as overridden by a workspace that had not touched it.
    const global = provider({ id: "ov", models: [{ id: "gpt-4o" }] });
    const effective = provider({ id: "ov", models: [{ id: "gpt-4o" }] });
    expect(overrideOf(global, [effective])).toEqual([]);
  });

  it("is named as an override when the workspace really did change it", () => {
    const global = provider({ id: "ov", models: [{ id: "gpt-4o" }] });
    const effective = provider({ id: "ov", models: [{ id: "o3" }] });
    expect(overrideOf(global, [effective])).toEqual(["models"]);
  });

  it("says what an empty list means rather than showing an empty box", () => {
    // Empty is the normal state, and "we will ask the endpoint" is not
    // something a user can infer from nothing being there.
    const html = renderToStaticMarkup(<ModelList models={[]} onChange={() => {}} />);
    expect(html).toContain("asks this endpoint what it serves");
  });

  it("gives every row's controls a label naming the model they belong to", () => {
    // Four identical unlabelled boxes per row is unusable with a screen
    // reader, and the id is the only thing telling them apart.
    const html = renderToStaticMarkup(
      <ModelList models={[{ id: "gpt-4o" }]} onChange={() => {}} />,
    );
    expect(html).toContain("Context length for gpt-4o");
    expect(html).toContain("Tool calling for gpt-4o");
    expect(html).toContain("Remove gpt-4o");
  });

  it("falls back to a position when a row has no id typed yet", () => {
    const html = renderToStaticMarkup(
      <ModelList models={[{ id: "" }]} onChange={() => {}} />,
    );
    expect(html).toContain("Remove model 1");
  });

  it("says the overrides are optional once there is a row to override", () => {
    const html = renderToStaticMarkup(
      <ModelList models={[{ id: "gpt-4o" }]} onChange={() => {}} />,
    );
    expect(html).toContain("replace whatever the endpoint would list");
  });
});

describe("adding a provider", () => {
  it("does not collide with an id already in use", () => {
    const first = blankProvider([]);
    const second = blankProvider([first]);
    const third = blankProvider([first, second]);
    expect(new Set([first.id, second.id, third.id]).size).toBe(3);
  });

  it("starts empty rather than with a plausible-looking default", () => {
    // A pre-filled base URL that happens to be wrong is harder to notice than
    // an empty one that validation rejects.
    expect(blankProvider([]).base_url).toBe("");
  });
});

describe("rendering", () => {
  it("survives a first paint with no status loaded yet", () => {
    // The drawer can be opened before `get_status` has resolved; every field
    // it shows has to tolerate a null status rather than throw.
    const html = renderToStaticMarkup(<Settings onClose={() => {}} />);
    expect(html).toContain("Models");
    expect(html).toContain("Permissions");
    expect(html).toContain("Behavior");
    // The paths block reads `status.workspace`, which is the field most
    // likely to be dereferenced before it exists.
    expect(html).toContain("This project");
  });
});

describe("statusHint", () => {
  const settings = (patch: Partial<SearchSettings> = {}): SearchSettings => ({
    selected: null,
    backends: [],
    key_statuses: [],
    active: false,
    problems: [],
    ...patch,
  });

  it("calls off what is off, without dressing it as a problem", () => {
    // Not searching is the default and a legitimate choice, so this must not
    // read like something the user has failed to finish.
    expect(statusHint(settings())).toBe("Off. Taurus will not search the web.");
  });

  it("says it is on only when the tools are actually registered", () => {
    expect(statusHint(settings({ selected: "brave", active: true }))).toBe(
      "On, through brave.",
    );
  });

  it("distinguishes a backend that is picked from one that runs", () => {
    // The state a missing key leaves behind. Reporting this as "on" would be
    // contradicted by the very next search that fails.
    const hint = statusHint(settings({ selected: "brave", active: false }));
    expect(hint).toContain("not running yet");
    expect(hint).toContain("brave");
  });
});

describe("which settings each provider kind shows", () => {
  it("offers every kind the harness can build", () => {
    // A kind in the config enum with no row here falls back to the
    // OpenAI-compatible layout, which shows a header and prefix that adapter
    // ignores.
    expect(Object.keys(FIELDS).sort()).toEqual([
      "anthropic",
      "gemini",
      "ollama",
      "open_ai_compatible",
    ]);
  });

  it("asks Ollama for nothing it answers for itself", () => {
    expect(FIELDS.ollama.key).toBe(false);
    expect(FIELDS.ollama.declareContext).toBe(false);
    expect(FIELDS.ollama.declareTools).toBe(false);
  });

  it("offers a ceiling on the one number Ollama reports that is not the answer", () => {
    // It reports the window the model was trained for. What the machine in
    // front of it can serve at a usable speed is a different question, and
    // nothing on the wire answers it — so the field is a cap rather than a
    // declaration, and it is the only kind that gets one.
    expect(FIELDS.ollama.contextCap).toBe(true);
    expect(FIELDS.ollama.declareContext).toBe(false);
    expect(FIELDS.ollama.contextFallback).toBe(false);
    for (const kind of ["anthropic", "gemini", "open_ai_compatible"] as const) {
      expect(FIELDS[kind].contextCap).toBe(false);
    }
  });

  it("does not offer a key header or prefix where the route is fixed", () => {
    // Ollama is local and unauthenticated; Gemini reads `x-goog-api-key` and
    // has no gateway support here yet. A field to change either is a setting
    // someone will change and then wonder about.
    expect(FIELDS.ollama.routing).toBeNull();
    expect(FIELDS.gemini.routing).toBeNull();
  });

  it("offers them wherever a gateway can sit in front", () => {
    // Anthropic used to be in the list above, on the grounds that the key
    // always rides `x-api-key`. True of api.anthropic.com and false of an
    // Azure APIM route, which reads its own subscription key and publishes the
    // API under a path of its own — so the adapter forced a header the gateway
    // rejects and a `/v1` that 404s.
    expect(FIELDS.anthropic.routing).not.toBeNull();
    expect(FIELDS.open_ai_compatible.routing).not.toBeNull();
  });

  it("names each kind's own defaults rather than one adapter's", () => {
    // The hint is what tells someone what leaving the field blank does, and
    // the answer is different per backend. Shown over the wrong field it is
    // worse than no hint.
    expect(FIELDS.anthropic.routing?.header).toBe("x-api-key");
    expect(FIELDS.open_ai_compatible.routing?.header).toBe(
      "Authorization: Bearer",
    );
    for (const kind of ["anthropic", "open_ai_compatible"] as const) {
      const routing = FIELDS[kind].routing!;
      expect(routing.headerHint).toContain(routing.header);
      expect(routing.prefixHint).toContain(routing.prefix);
    }
  });

  it("does not ask a probing backend to declare what it reports", () => {
    // Both new adapters read their own context window per model. A declared
    // one that disagrees is how a conversation compacts at the wrong moment,
    // so it is offered only as a fallback.
    expect(FIELDS.anthropic.declareContext).toBe(false);
    expect(FIELDS.anthropic.contextFallback).toBe(true);
    expect(FIELDS.gemini.contextFallback).toBe(true);
    expect(FIELDS.open_ai_compatible.declareContext).toBe(true);
  });

  it("asks about images only where the answer is in doubt", () => {
    // Ollama reports vision per model, and every model Anthropic and Gemini
    // serve reads images. Only an OpenAI-compatible endpoint might be fronting
    // text-only weights with no way to say so.
    expect(FIELDS.open_ai_compatible.declareVision).toBe(true);
    expect(FIELDS.ollama.declareVision).toBe(false);
    expect(FIELDS.anthropic.declareVision).toBe(false);
    expect(FIELDS.gemini.declareVision).toBe(false);
  });

  it("shows the thinking setting only where one exists", () => {
    expect(FIELDS.anthropic.thinking).toBe(true);
    expect(FIELDS.gemini.thinking).toBe(false);
    expect(FIELDS.ollama.thinking).toBe(false);
  });
});
