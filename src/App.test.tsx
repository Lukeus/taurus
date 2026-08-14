import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

/**
 * The store, stubbed.
 *
 * zustand v5 serves `getInitialState` as the SSR snapshot, so `setState` cannot
 * drive a static render — the header would always be drawn against an empty
 * store. Replacing the hook is what lets it be rendered against a chosen set of
 * providers.
 */
const state = {
  status: null as unknown,
  session: null as unknown,
  sessions: [],
  entries: [],
  changed: [],
  proposals: [],
  agentProposals: [],
  busy: false,
  error: null,
  init: vi.fn(),
  startSession: vi.fn(),
  resume: vi.fn(),
  setWorkspace: vi.fn(),
  refresh: vi.fn(),
  reload: vi.fn(),
  send: vi.fn(),
  stop: vi.fn(),
  dismissError: vi.fn(),
};
vi.mock("./state/store", () => ({ useStore: () => state }));

import type { ProviderConfig } from "./lib/api";
import App, { currentProvider, offered } from "./App";

const provider = (id: string): ProviderConfig => ({
  id,
  kind: "open_ai_compatible",
  base_url: `http://${id}`,
  models: [],
  default_model: null,
  api_key_env: null,
  api_key_header: null,
  native_tools: null,
  context_length: null,
  api_prefix: null,
});

const configured = [provider("ollama"), provider("openai"), provider("azure")];

describe("which provider the header shows", () => {
  it("follows the open conversation, whichever provider that is", () => {
    // The bug this replaced: the header pinned itself to the first configured
    // provider, so a conversation running on the third looked like the first.
    expect(currentProvider(configured, "azure", "openai")).toBe("azure");
  });

  it("falls back to the one this workspace was last worked in", () => {
    expect(currentProvider(configured, undefined, "openai")).toBe("openai");
  });

  it("does not pin to the first provider just because it is first", () => {
    // Ollama is seeded on first run and so is always index 0; a user who added
    // and chose another provider must not be dragged back to it.
    expect(currentProvider(configured, undefined, "azure")).not.toBe("ollama");
  });

  it("uses the first provider when nothing was remembered", () => {
    expect(currentProvider(configured, undefined, null)).toBe("ollama");
  });

  it("uses the first provider when the remembered one has been removed", () => {
    expect(currentProvider(configured, undefined, "deleted")).toBe("ollama");
  });

  it("reports nothing when no providers are configured", () => {
    expect(currentProvider([], undefined, "openai")).toBeUndefined();
  });
});

describe("what the model picker offers", () => {
  const listed = [
    { id: "gpt-4o", display_name: "gpt-4o", context_length: null },
    { id: "o3", display_name: "o3", context_length: null },
  ];

  it("shows what the backend listed, when it listed anything", () => {
    expect(offered(listed, provider("openai")).map((m) => m.id)).toEqual([
      "gpt-4o",
      "o3",
    ]);
  });

  it("falls back to the models the config names when the listing fails", () => {
    // The reported bug: a gateway with no /v1/models route left the picker
    // saying "no models" while a conversation ran on one of them.
    const gateway = {
      ...provider("apim"),
      models: [{ id: "gpt-4o" }, { id: "o3" }],
    };
    expect(offered("failed", gateway).map((m) => m.id)).toEqual(["gpt-4o", "o3"]);
  });

  it("labels a configured model by its display name when it has one", () => {
    const gateway = {
      ...provider("apim"),
      models: [{ id: "llama-3.1-8b", display_name: "Llama 3.1 8B" }],
    };
    expect(offered("failed", gateway)[0].display_name).toBe("Llama 3.1 8B");
  });

  it("still offers a lone default_model, which is all older configs have", () => {
    const gateway = { ...provider("apim"), default_model: "gpt-4o" };
    expect(offered("failed", gateway).map((m) => m.id)).toEqual(["gpt-4o"]);
  });

  it("keeps the running model selectable even when nothing listed it", () => {
    // Otherwise the <select> has no matching option and displays its first
    // one, naming a model the conversation is not on.
    const gateway = { ...provider("apim"), models: [{ id: "gpt-4o" }] };
    const ids = offered("failed", gateway, "retired-model").map((m) => m.id);
    expect(ids).toContain("retired-model");
    expect(ids).toContain("gpt-4o");
  });

  it("does not repeat the running model when the list already has it", () => {
    expect(offered(listed, provider("openai"), "o3")).toHaveLength(2);
  });

  it("offers nothing when there is nothing to offer", () => {
    expect(offered("failed", provider("apim"))).toEqual([]);
    expect(offered(null, undefined)).toEqual([]);
  });
});

const withProviders = (ids: string[], lastProvider: string | null = null) => {
  state.status = {
    workspace: "/w",
    providers: ids.map(provider),
    settings: {
      last_workspace: null,
      last_provider: lastProvider,
      last_model: null,
      skill_synthesis_enabled: true,
    },
    skill_count: 0,
    problems: [],
    tool_names: [],
    mcp_servers: [],
  };
  return renderToStaticMarkup(<App />);
};

const providerSelect = (html: string) =>
  html.match(/<select[^>]*class="provider-select"[\s\S]*?<\/select>/)?.[0];

describe("the provider picker in the header", () => {
  it("lists every configured provider", () => {
    const select = providerSelect(withProviders(["ollama", "openai", "azure"]));
    expect(select).toBeDefined();
    for (const id of ["ollama", "openai", "azure"]) {
      expect(select).toContain(`value="${id}"`);
    }
  });

  it("selects the remembered provider rather than the first", () => {
    // Without this the picker drew the right options and still pointed at the
    // wrong one, which is the bug wearing a dropdown.
    const select = providerSelect(withProviders(["ollama", "azure"], "azure"));
    expect(select).toMatch(/<option value="azure" selected=""/);
  });

  it("is absent when there is only one provider to choose from", () => {
    expect(providerSelect(withProviders(["ollama"]))).toBeUndefined();
  });
});
