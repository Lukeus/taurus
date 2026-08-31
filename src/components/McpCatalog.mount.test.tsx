// @vitest-environment jsdom
//
// Browsing the catalogue, and filling one of its entries in. Both are stateful
// — the list arrives from a command, the PATH answer from a second one, and the
// setup form's Continue depends on what has been typed — so neither can be seen
// in a first paint.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { McpCatalog } from "./McpCatalog";
import { McpSetup } from "./McpSetup";
import type { CatalogEntry, McpServerDraft, McpServerView } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  invoke.mockReset();
});

const FILESYSTEM: CatalogEntry = {
  id: "filesystem",
  name: "Filesystem",
  blurb: "Read and write files under directories you name.",
  homepage: "https://example.invalid/filesystem",
  keywords: ["files", "folder"],
  scope: "project",
  requires: "npx",
  transport: "stdio",
  command: "npx",
  args: ["-y", "@modelcontextprotocol/server-filesystem", "{directory}"],
  env: [],
  url: "",
  headers: [],
  inputs: [
    {
      key: "directory",
      label: "Directory it may reach",
      help: "An absolute path the server refuses to look outside.",
      kind: "path",
      required: true,
    },
  ],
};

const GIT: CatalogEntry = {
  ...FILESYSTEM,
  id: "git",
  name: "Git",
  blurb: "Read history and diffs.",
  keywords: ["git"],
  requires: "uvx",
  command: "uvx",
  args: ["mcp-server-git"],
  inputs: [],
};

const GITHUB: CatalogEntry = {
  ...FILESYSTEM,
  id: "github",
  name: "GitHub",
  blurb: "Issues and pull requests.",
  keywords: ["github"],
  scope: "global",
  requires: undefined,
  transport: "http",
  command: "",
  args: [],
  url: "https://api.githubcopilot.com/mcp/",
  headers: [{ key: "Authorization", value: "Bearer {token}" }],
  inputs: [
    {
      key: "token",
      label: "Personal access token",
      help: "A fine-grained token scoped to what you want reachable.",
      kind: "secret",
      required: true,
    },
  ],
};

const POSTGRES: CatalogEntry = {
  ...FILESYSTEM,
  id: "postgres",
  name: "PostgreSQL",
  blurb: "Query a Postgres database.",
  keywords: ["sql", "db"],
  requires: undefined,
  blocked:
    "There is no first-party Postgres server to recommend — the reference one was withdrawn over a SQL-injection vulnerability.",
  command: "",
  args: [],
  inputs: [],
};

const CATALOG = {
  revised: "2026-08-31",
  entries: [FILESYSTEM, GIT, GITHUB, POSTGRES],
};

/** Mounts the catalogue and lets both of its commands settle. */
async function browse(
  installed: McpServerView[] = [],
  onPath: string[] = ["npx", "uvx"],
) {
  invoke.mockImplementation((command: string) => {
    if (command === "mcp_catalog") return Promise.resolve(CATALOG);
    if (command === "programs_on_path") return Promise.resolve(onPath);
    return Promise.resolve(undefined);
  });

  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const picked: CatalogEntry[] = [];
  await act(async () => {
    root.render(
      <McpCatalog
        installed={installed}
        onPick={(entry) => picked.push(entry)}
        onBack={() => {}}
      />,
    );
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });

  const cards = () => [...host.querySelectorAll<HTMLElement>(".catalog-card")];
  return {
    host,
    picked,
    cards,
    card: (name: string) =>
      cards().find((c) => c.querySelector(".card-title")?.textContent === name)!,
    search: async (text: string) =>
      act(async () => {
        const box = host.querySelector("input") as HTMLInputElement;
        const set = Object.getOwnPropertyDescriptor(
          HTMLInputElement.prototype,
          "value",
        )!.set!;
        set.call(box, text);
        box.dispatchEvent(new Event("input", { bubbles: true }));
      }),
  };
}

describe("browsing the catalogue", () => {
  it("asks the PATH only about programs the list actually names", async () => {
    // Rather than a fixed pair, so an entry added later needing `docker` gets
    // its warning without anybody remembering to widen this call.
    await browse();
    const [, args] = invoke.mock.calls.find(
      ([name]) => name === "programs_on_path",
    )!;
    expect((args as { names: string[] }).names.sort()).toEqual(["npx", "uvx"]);
  });

  it("warns about a fetcher it cannot see before anything is filled in", async () => {
    // The question behind almost every stdio failure, and the whole point of
    // asking it here: "uvx is not on the PATH" has to arrive before somebody
    // types a connection string, not after the server refuses to start.
    const { card } = await browse([], ["npx"]);
    expect(card("Git").textContent).toContain("needs uvx");
    expect(card("Filesystem").textContent).not.toContain("needs");
  });

  it("says nothing about the PATH until the answer has landed", async () => {
    // A warning that might be wrong is worse than no warning yet.
    invoke.mockImplementation((command: string) =>
      command === "mcp_catalog"
        ? Promise.resolve(CATALOG)
        : new Promise(() => {}),
    );
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => {
      root.render(
        <McpCatalog installed={[]} onPick={() => {}} onBack={() => {}} />,
      );
    });
    cleanup.push(() => {
      act(() => root.unmount());
      host.remove();
    });
    expect(host.textContent).not.toContain("needs");
  });

  it("matches a word nobody would find in the name", async () => {
    const { cards, search } = await browse();
    await search("db");
    expect(cards()).toHaveLength(1);
    expect(cards()[0].textContent).toContain("PostgreSQL");
  });

  it("explains what it cannot offer instead of offering it", async () => {
    /*
     * Half the reason this is a curated list rather than a registry search.
     * Searching for the thing you cannot have returns why, and a blocked entry
     * gets no button at all — a greyed-out Install would read as "not yet",
     * where these are mostly "not ever, by this route".
     */
    const { card } = await browse();
    const postgres = card("PostgreSQL");
    expect(postgres.textContent).toContain("SQL-injection");
    expect(postgres.querySelector("button")).toBeNull();
  });

  it("does not call a blocked entry added, even when something of that name is", async () => {
    // "Added" beside "there is no server here to recommend" reads as a
    // contradiction, and the paragraph is the only thing on that card worth
    // reading.
    const installed = [{ name: "postgres" } as McpServerView];
    const { card } = await browse(installed);
    expect(card("PostgreSQL").textContent).not.toContain("added");
  });

  it("says which entries are already configured", async () => {
    const installed = [{ name: "filesystem" } as McpServerView];
    const { card } = await browse(installed);
    expect(card("Filesystem").textContent).toContain("added");
    expect(card("Filesystem").textContent).toContain("Add another");
    expect(card("Git").textContent).not.toContain("added");
  });

  it("links the source on every card, blocked ones included", async () => {
    // The answer to "a package name says nothing about what the program does",
    // and the least a list like this owes the person reading it.
    const { cards } = await browse();
    for (const card of cards()) {
      expect(card.querySelector("a")?.getAttribute("href")).toMatch(/^https:/);
    }
  });

  it("hands the chosen entry back rather than installing it", async () => {
    const { card, picked } = await browse();
    await act(async () => {
      card("Git").querySelector("button")!.click();
    });
    expect(picked.map((e) => e.id)).toEqual(["git"]);
    // Nothing was written. Everything goes through the ordinary editor.
    expect(invoke.mock.calls.some(([name]) => name === "save_mcp_server")).toBe(
      false,
    );
  });
});

/** Mounts the setup step for one entry. */
function setup(entry: CatalogEntry) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const ready: McpServerDraft[] = [];
  act(() => {
    root.render(
      <McpSetup entry={entry} onReady={(d) => ready.push(d)} onCancel={() => {}} />,
    );
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });

  const button = (label: string) =>
    [...host.querySelectorAll("button")].find(
      (b) => b.textContent === label,
    ) as HTMLButtonElement;

  return {
    host,
    ready,
    button,
    type: (value: string) =>
      act(() => {
        const box = host.querySelector("input") as HTMLInputElement;
        const set = Object.getOwnPropertyDescriptor(
          HTMLInputElement.prototype,
          "value",
        )!.set!;
        set.call(box, value);
        box.dispatchEvent(new Event("input", { bubbles: true }));
      }),
    choose: (value: string) =>
      act(() => {
        const select = host.querySelector("select") as HTMLSelectElement;
        const set = Object.getOwnPropertyDescriptor(
          HTMLSelectElement.prototype,
          "value",
        )!.set!;
        set.call(select, value);
        select.dispatchEvent(new Event("change", { bubbles: true }));
      }),
    tick: () =>
      act(() => {
        (host.querySelector(
          '.mcp-leak input[type="checkbox"]',
        ) as HTMLInputElement).click();
      }),
  };
}

describe("filling an entry in", () => {
  it("will not continue until the required boxes have something in them", () => {
    // An entry that installed `npx -y …server-filesystem` with no directory
    // would refuse every path it was handed, and the failure would arrive in a
    // tool call wearing no connection to the button that caused it.
    const form = setup(FILESYSTEM);
    expect(form.button("Continue").disabled).toBe(true);
    form.type("/src/taurus");
    expect(form.button("Continue").disabled).toBe(false);
  });

  it("hands over a draft rather than saving one", () => {
    const form = setup(FILESYSTEM);
    form.type("/src/taurus");
    act(() => form.button("Continue").click());

    expect(form.ready).toHaveLength(1);
    expect(form.ready[0].args).toEqual([
      "-y",
      "@modelcontextprotocol/server-filesystem",
      "/src/taurus",
    ]);
    expect(invoke.mock.calls.some(([n]) => n === "save_mcp_server")).toBe(false);
  });

  it("shows the command before it is committed to", () => {
    // Pressing Continue must never be the first time the command line has been
    // on screen.
    const form = setup(FILESYSTEM);
    form.type("/src/taurus");
    expect(form.host.querySelector(".mcp-preview")?.textContent).toContain(
      "@modelcontextprotocol/server-filesystem /src/taurus",
    );
  });

  it("keeps a credential out of the preview", () => {
    const form = setup(GITHUB);
    form.type("ghp_realtokenvalue");
    const preview = form.host.querySelector(".mcp-preview")!.textContent ?? "";
    expect(preview).toContain("https://api.githubcopilot.com/mcp/");
    expect(preview).not.toContain("ghp_realtokenvalue");
  });

  it("defaults a credential to the file that is not inside a repository", () => {
    const form = setup(GITHUB);
    expect((form.host.querySelector("select") as HTMLSelectElement).value).toBe(
      "global",
    );
    expect(form.host.querySelector(".mcp-leak")).toBeNull();
  });

  it("makes writing one into the workspace a decision rather than a default", () => {
    /*
     * The one footgun the catalogue can catch that somebody filling in a form
     * cannot: `<workspace>/.taurus/mcp.json` is a file in a repository, and the
     * commit that publishes the token is one `git add .` away. Not refused —
     * there are good reasons to want it — but not defaulted into either.
     */
    const form = setup(GITHUB);
    form.type("ghp_realtokenvalue");
    form.choose("workspace");

    expect(form.host.querySelector(".mcp-leak")).not.toBeNull();
    expect(form.button("Continue").disabled).toBe(true);
    form.tick();
    expect(form.button("Continue").disabled).toBe(false);
  });

  it("re-asks when the scope is changed back into the warning", () => {
    // Acknowledging once must not silently cover a later switch.
    const form = setup(GITHUB);
    form.type("ghp_realtokenvalue");
    form.choose("workspace");
    form.tick();
    form.choose("global");
    form.choose("workspace");
    expect(form.button("Continue").disabled).toBe(true);
  });

  it("says nothing about scope for an entry with no credential", () => {
    const form = setup(FILESYSTEM);
    form.type("/src/taurus");
    expect(form.host.querySelector(".mcp-leak")).toBeNull();
  });
});
