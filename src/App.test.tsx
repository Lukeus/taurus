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
import App, { currentProvider } from "./App";

const provider = (id: string): ProviderConfig => ({
  id,
  kind: "open_ai_compatible",
  base_url: `http://${id}`,
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
    skill_problems: [],
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
