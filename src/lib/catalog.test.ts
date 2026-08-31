// Turning a catalogue entry into an entry in `mcp.json`.
//
// The whole value of the catalogue is that it knows things the person adding a
// server does not — which argument the directory goes in, which header carries
// the token — so the substitution is the part that has to be right. Every
// failure here reaches the user as a server that will not start, wearing no
// connection to the button that installed it.
import { describe, expect, it } from "vitest";

import type { CatalogEntry } from "./api";
import {
  fromCatalog,
  leaksSecret,
  missingInputs,
  scopeFor,
  searchCatalog,
} from "./mcp";

const entry = (patch: Partial<CatalogEntry> = {}): CatalogEntry => ({
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
      help: "An absolute path.",
      kind: "path",
      required: true,
    },
  ],
  ...patch,
});

const http = (patch: Partial<CatalogEntry> = {}): CatalogEntry =>
  entry({
    id: "github",
    name: "GitHub",
    keywords: ["issues", "pr", "repo"],
    transport: "http",
    requires: undefined,
    command: "",
    args: [],
    url: "https://api.githubcopilot.com/mcp/",
    headers: [{ key: "Authorization", value: "Bearer {token}" }],
    scope: "global",
    inputs: [
      {
        key: "token",
        label: "Personal access token",
        help: "A fine-grained token.",
        kind: "secret",
        required: true,
      },
    ],
    ...patch,
  });

describe("filling in an entry", () => {
  it("puts an answer where the template says it goes", () => {
    const draft = fromCatalog(entry(), { directory: "/src/taurus" }, "workspace");
    expect(draft.command).toBe("npx");
    expect(draft.args).toEqual([
      "-y",
      "@modelcontextprotocol/server-filesystem",
      "/src/taurus",
    ]);
  });

  it("fills a header as readily as an argument", () => {
    const draft = fromCatalog(http(), { token: "ghp_live" }, "global");
    expect(draft.url).toBe("https://api.githubcopilot.com/mcp/");
    expect(draft.headers).toEqual([
      { key: "Authorization", value: "Bearer ghp_live", secret: false },
    ]);
    // The other transport's fields are left empty rather than half-filled: a
    // draft carrying both is one the backend has to guess about.
    expect(draft.command).toBe("");
    expect(draft.args).toEqual([]);
  });

  it("drops an argument whose optional answer was left blank", () => {
    /*
     * The case that decides whether optional inputs are usable at all. Time
     * takes `--local-timezone={timezone}`, and leaving the box empty has to
     * remove the argument — writing it through with the braces intact makes the
     * server take `{timezone}` as a zone name, and writing it empty makes it
     * refuse to start. Neither failure points back at this box.
     */
    const time = entry({
      id: "time",
      command: "uvx",
      args: ["mcp-server-time", "--local-timezone={timezone}"],
      inputs: [
        {
          key: "timezone",
          label: "Local timezone",
          help: "An IANA name.",
          kind: "text",
          required: false,
        },
      ],
    });
    expect(fromCatalog(time, {}, "global").args).toEqual(["mcp-server-time"]);
    expect(fromCatalog(time, { timezone: "  " }, "global").args).toEqual([
      "mcp-server-time",
    ]);
    expect(
      fromCatalog(time, { timezone: "Europe/London" }, "global").args,
    ).toEqual(["mcp-server-time", "--local-timezone=Europe/London"]);
  });

  it("drops a header whose value was left blank rather than sending it empty", () => {
    // `Authorization: Bearer` fails with a confusing 401. No header at all
    // fails with whatever the server actually means.
    expect(fromCatalog(http(), {}, "global").headers).toEqual([]);
  });

  it("never leaves a placeholder in what it writes", () => {
    // The failure this whole shape exists to prevent: `{token}` sitting in a
    // config file, literally, as a credential.
    const draft = fromCatalog(http(), {}, "global");
    expect(JSON.stringify(draft)).not.toContain("{token}");
  });
});

describe("what an entry still needs", () => {
  it("names the required boxes that are empty", () => {
    expect(missingInputs(entry(), {})).toEqual(["Directory it may reach"]);
    expect(missingInputs(entry(), { directory: "/src" })).toEqual([]);
  });

  it("counts whitespace as empty", () => {
    expect(missingInputs(entry(), { directory: "   " })).toHaveLength(1);
  });

  it("does not ask for the optional ones", () => {
    const optional = entry({
      inputs: [
        {
          key: "directory",
          label: "Directory",
          help: "An absolute path.",
          kind: "path",
          required: false,
        },
      ],
    });
    expect(missingInputs(optional, {})).toEqual([]);
  });
});

describe("where a credential would land", () => {
  it("catches a secret being written into the workspace file", () => {
    /*
     * The one footgun the catalogue can catch that somebody filling in a form
     * cannot. Project scope is `<workspace>/.taurus/mcp.json` — a file inside
     * a repository — and the commit that leaks the token is one `git add .`
     * away. Every shipped entry wanting a secret defaults to global; this is
     * what happens when that is changed by hand.
     */
    expect(leaksSecret(http(), "workspace")).toBe(true);
    expect(leaksSecret(http(), "global")).toBe(false);
  });

  it("says nothing about an entry with no credential in it", () => {
    expect(leaksSecret(entry(), "workspace")).toBe(false);
  });

  it("maps the catalogue's word for the layer onto the harness's", () => {
    // The catalogue says `project`, the config files say `workspace`. One
    // layer, two names, mapped in exactly one place.
    expect(scopeFor(entry())).toBe("workspace");
    expect(scopeFor(http())).toBe("global");
  });
});

describe("searching it", () => {
  const all = [
    entry(),
    http(),
    entry({
      id: "postgres",
      name: "PostgreSQL",
      keywords: ["sql", "database", "db"],
      blocked: "There is no first-party server to recommend.",
      command: "",
      args: [],
      inputs: [],
    }),
  ];

  it("matches a word nobody would find in the name", () => {
    // What gets typed is rarely the name.
    expect(searchCatalog(all, "db").map((e) => e.id)).toEqual(["postgres"]);
    expect(searchCatalog(all, "folder").map((e) => e.id)).toEqual(["filesystem"]);
  });

  it("returns the reason for something that cannot be installed", () => {
    // The point of carrying blocked entries at all: searching for the thing
    // you cannot have should return why, not nothing.
    const [found] = searchCatalog(all, "postgres");
    expect(found.blocked).toMatch(/first-party/);
  });

  it("returns everything for an empty query", () => {
    expect(searchCatalog(all, "  ")).toHaveLength(3);
  });
});
