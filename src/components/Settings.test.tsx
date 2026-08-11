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
  Settings,
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
  default_model: null,
  api_key_env: null,
  api_key_header: null,
  native_tools: null,
  context_length: null,
  api_prefix: null,
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
