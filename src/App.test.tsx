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
  datasets: [] as unknown[],
  busy: false,
  stopping: false,
  error: null,
  init: vi.fn(),
  startSession: vi.fn(),
  resume: vi.fn(),
  setWorkspace: vi.fn(),
  refresh: vi.fn(),
  reload: vi.fn(),
  send: vi.fn(),
  stop: vi.fn(),
  refreshDatasets: vi.fn(),
  forgetDataset: vi.fn(),
  dismissError: vi.fn(),
};
// `pinnedPlan` is a pure selector over `entries`, which is empty here — the
// real one is exercised in the store's own tests, and stubbing it would only
// let App call something that does not exist.
//
// The selector is applied rather than ignored, because App and the two panes
// under it subscribe to slices rather than to the whole store. A mock that
// handed every caller the entire state would give `Transcript` the store object
// where it expects a list of entries.
vi.mock("./state/store", async (original) => ({
  ...(await original<typeof import("./state/store")>()),
  useStore: (select?: (s: typeof state) => unknown) =>
    select ? select(state) : state,
}));

import type { ProviderConfig } from "./lib/api";
import type { Entry } from "./state/store";
import App, {
  currentProvider,
  lastActivity,
  offered,
  isAsking,
  onScreenFor,
  withDraft,
  TurnStrip,
} from "./App";

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
  vision: null,
  api_prefix: null,
  thinking: null,
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
    dataset_count: 0,
    problems: [],
    tool_names: [],
    mcp_servers: [],
  };
  return renderToStaticMarkup(<App />);
};

/**
 * The rule the whole surface rests on: no dataset, no switch.
 *
 * This is how the Data pane stays out of the way of everybody not using it —
 * the same discipline as the composer only announcing `/` when there is
 * something to run, and the rail's MCP badge staying absent until a status
 * lands. A tab that was always there would be a permanent advertisement for a
 * feature most workspaces have nothing to put in.
 */
describe("the Data switch", () => {
  const render = (datasets: unknown[]) => {
    state.datasets = datasets;
    state.status = {
      workspace: "/w",
      providers: [provider("ollama")],
      settings: {
        last_workspace: null,
        last_provider: null,
        last_model: null,
        skill_synthesis_enabled: true,
      },
      skill_count: 0,
      dataset_count: datasets.length,
      problems: [],
      tool_names: [],
      mcp_servers: [],
    };
    const html = renderToStaticMarkup(<App />);
    state.datasets = [];
    return html;
  };

  it("is absent in a workspace that has loaded nothing", () => {
    expect(render([])).not.toContain("pane-switch");
  });

  it("appears once there is something behind it, and counts it", () => {
    const html = render([
      { name: "events", path: "data/events.csv", format: "csv" },
      { name: "items", path: "data/items.parquet", format: "parquet" },
    ]);
    expect(html).toContain("pane-switch");
    expect(html).toContain("Conversation");
    expect(html).toContain("Data");
    expect(html).toContain(">2<");
  });
});

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

/**
 * The composer sits under the Data pane as well as under the transcript, so a
 * message can be about a table nobody named. This is what gives "this" a
 * referent — and what keeps it from arriving where it would be noise.
 */
describe("what a message carries from the pane it was sent in", () => {
  const events = { name: "events", path: "data/events.csv", format: "csv" as const };

  it("carries nothing from the conversation", () => {
    // A question asked while reading a conversation is about the conversation.
    expect(onScreenFor("conversation", events, "SELECT 1")).toBeNull();
  });

  it("carries the dataset and its file from the pane", () => {
    expect(onScreenFor("data", events, "")).toEqual({
      dataset: "events",
      path: "data/events.csv",
    });
  });

  /** "Why does this not work?" is a question about the text in the box. */
  it("carries the query box when there is something in it", () => {
    expect(onScreenFor("data", events, " SELECT count(*) FROM events ")?.sql).toBe(
      "SELECT count(*) FROM events",
    );
  });

  it("leaves an empty box out rather than sending an empty string", () => {
    expect(onScreenFor("data", events, "   \n ")).not.toHaveProperty("sql");
  });

  it("carries nothing when the pane is open on nothing", () => {
    expect(onScreenFor("data", null, "SELECT 1")).toBeNull();
  });
});

describe("whether the turn is waiting on an answer", () => {
  const asking = (status: "running" | "ok"): Entry => ({
    kind: "tool",
    id: "q1",
    name: "ask_user",
    preview: "Ask 2 questions",
    status,
    steps: [],
    view: { type: "questions", id: "q1", questions: [] },
  });

  /*
   * The gap this closes: a question card parks the turn, and the strip went on
   * saying `Ask 2 questions` with a progress cadence under it — a turn that
   * has been waiting a minute looked exactly like one that was working.
   */
  it("notices a question card that is still waiting", () => {
    expect(isAsking([asking("running")])).toBe(true);
  });

  it("does not, once it has been answered", () => {
    expect(isAsking([asking("ok")])).toBe(false);
  });

  it("does not for an ordinary running call", () => {
    expect(
      isAsking([
        {
          kind: "tool",
          id: "t1",
          name: "read_file",
          preview: "Read src/main.rs",
          status: "running",
          steps: [],
        },
      ]),
    ).toBe(false);
  });

  it("reads the last entry only, because a parked call is the newest thing", () => {
    // The harness runs one call at a time and this one is blocking it, so
    // anything after it means it is no longer what the turn is waiting on.
    expect(
      isAsking([
        asking("running"),
        { kind: "assistant", id: "a1", text: "Thanks.", thinking: "", open: false },
      ]),
    ).toBe(false);
    expect(isAsking([])).toBe(false);
  });
});

describe("a draft put in the composer from elsewhere", () => {
  const ASK = "This query fails:\n\n```sql\nSELECT 1\n```\n\nWhat should it be?";

  it("fills an empty box with exactly what was offered", () => {
    expect(withDraft("", ASK)).toBe(ASK);
    expect(withDraft("  \n ", ASK)).toBe(ASK);
  });

  it("adds to a half-written question rather than replacing it", () => {
    // The buttons that offer these sit three inches from the composer, and
    // both are places you may have started typing before something failed.
    // Throwing that away to make room for a canned sentence is the app
    // deciding it knows better.
    expect(withDraft("and only for EU", ASK)).toBe(`and only for EU\n\n${ASK}`);
  });

  it("does not leave the join hanging on whitespace", () => {
    expect(withDraft("  ask me  \n\n", "second")).toBe("ask me\n\nsecond");
  });
});

describe("what the turn strip says", () => {
  const tool = (preview: string, status: "running" | "ok") => ({
    kind: "tool" as const,
    id: "t1",
    name: "query_data",
    preview,
    status,
    steps: [],
  });
  const said = (text: string) => ({
    kind: "assistant" as const,
    id: "a1",
    text,
    thinking: "",
    open: false,
  });

  it("says nothing it does not know rather than guessing", () => {
    expect(lastActivity([])).toBeNull();
  });

  /** A running tool is what the turn is doing *now*; the prose above it is
   *  what it was saying before it went and did something. */
  it("prefers a running tool to the prose before it", () => {
    expect(lastActivity([said("Let me look."), tool("Query: SELECT 1", "running")])).toBe(
      "Query: SELECT 1",
    );
  });

  /** The last sentence, not the first: on one line, the first is a paragraph
   *  old by the time anyone reads it. */
  it("takes the last sentence of what was said", () => {
    expect(lastActivity([said("I read the file. Nothing looks wrong here.")])).toBe(
      "Nothing looks wrong here.",
    );
  });

  it("falls back to a finished tool while nothing newer has happened", () => {
    expect(lastActivity([tool("Read data/events.csv", "ok")])).toBe(
      "Read data/events.csv",
    );
  });

  it("skips an assistant entry that is still empty", () => {
    expect(lastActivity([tool("Read a.csv", "ok"), said("   ")])).toBe("Read a.csv");
  });
});

describe("the turn strip", () => {
  const render = (entries: unknown[], stopping = false) => {
    state.entries = entries as never;
    state.stopping = stopping;
    const html = renderToStaticMarkup(<TurnStrip onOpen={() => {}} />);
    state.entries = [];
    state.stopping = false;
    return html;
  };

  it("shows what is happening, and the way back to where it is being said", () => {
    const html = render([
      {
        kind: "tool",
        id: "t1",
        name: "query_data",
        preview: "Query: SELECT count(*) FROM events",
        status: "running",
      },
    ]);
    expect(html).toContain("Query: SELECT count(*) FROM events");
    expect(html).toContain("Show the conversation");
  });

  /** Stop was pressed and the turn is unwinding. Saying what it was doing
   *  before that would read as though nothing had been heard. */
  it("says it is stopping rather than what it was doing", () => {
    const html = render(
      [{ kind: "assistant", id: "a1", text: "Reading it now.", thinking: "", open: false }],
      true,
    );
    expect(html).toContain("Stopping…");
    expect(html).not.toContain("Reading it now.");
  });

  it("says something rather than nothing before the first event arrives", () => {
    expect(render([])).toContain("Working…");
  });
});
