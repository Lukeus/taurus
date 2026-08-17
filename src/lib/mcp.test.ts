import { describe, expect, it } from "vitest";

import type { McpServerView } from "./api";
import {
  draftProblem,
  joinCommandLine,
  splitCommandLine,
  stateOf,
} from "./mcp";

const view = (patch: Partial<McpServerView> = {}): McpServerView => ({
  name: "filesystem",
  scope: "global",
  transport: "stdio",
  command: "npx",
  args: [],
  env: [],
  url: "",
  headers: [],
  disabled: false,
  ...patch,
});

describe("splitting a pasted command line", () => {
  it("splits the line people actually paste out of a README", () => {
    expect(
      splitCommandLine("npx -y @modelcontextprotocol/server-filesystem /tmp"),
    ).toEqual(["npx", "-y", "@modelcontextprotocol/server-filesystem", "/tmp"]);
  });

  it("keeps a quoted path together", () => {
    // The failure worth guarding: splitting on spaces turns one path into two
    // arguments, and the server starts somewhere else entirely — with no error,
    // because both halves are valid strings.
    expect(splitCommandLine('server --root "/Users/me/My Documents"')).toEqual([
      "server",
      "--root",
      "/Users/me/My Documents",
    ]);
    expect(splitCommandLine("server --root '/Users/me/My Documents'")).toEqual([
      "server",
      "--root",
      "/Users/me/My Documents",
    ]);
  });

  it("honours a backslash escape outside single quotes and not inside them", () => {
    expect(splitCommandLine("a b\\ c")).toEqual(["a", "b c"]);
    // Single quotes are literal, which is what makes a Windows path pasted
    // inside them survive.
    expect(splitCommandLine("a 'b\\c'")).toEqual(["a", "b\\c"]);
  });

  it("treats an empty quoted string as an argument", () => {
    // `--flag ""` means one empty argument. Dropping it changes the argument
    // list the program is handed.
    expect(splitCommandLine('cmd --flag ""')).toEqual(["cmd", "--flag", ""]);
  });

  it("collapses whatever whitespace a paste brought with it", () => {
    expect(splitCommandLine("  npx   -y \n pkg  ")).toEqual(["npx", "-y", "pkg"]);
    expect(splitCommandLine("   ")).toEqual([]);
  });

  it("round-trips through the joined form", () => {
    // The editor renders a saved entry back into the one field it was typed in,
    // so a value that does not survive the trip silently rewrites itself the
    // next time the form is opened.
    for (const parts of [
      ["npx", "-y", "pkg"],
      ["server", "--root", "/Users/me/My Documents"],
      ["odd", 'quote"inside', "back\\slash", ""],
    ]) {
      expect(splitCommandLine(joinCommandLine(parts))).toEqual(parts);
    }
  });
});

describe("what a row says about a server", () => {
  it("reads switched-off before anything else", () => {
    // A disabled server keeps whatever status it had before it was switched
    // off, and showing that would put a green dot on a server that is not
    // running.
    const off = stateOf(
      view({
        disabled: true,
        status: {
          name: "filesystem",
          description: "npx",
          connected: true,
          tool_count: 11,
          error: null,
          disabled: true,
          tools: [],
        },
      }),
    );
    expect(off).toEqual({ tone: "off", label: "off" });
  });

  it("distinguishes never-connected from failed", () => {
    // Before the first reload there is no answer yet, which is not the same as
    // a server that tried and could not start — and only one of those is worth
    // showing in red.
    expect(stateOf(view()).tone).toBe("unknown");
    expect(
      stateOf(
        view({
          status: {
            name: "filesystem",
            description: "npx",
            connected: false,
            tool_count: 0,
            error: "`npx` is not on this application's PATH",
            disabled: false,
            tools: [],
          },
        }),
      ),
    ).toEqual({
      tone: "error",
      label: "`npx` is not on this application's PATH",
    });
  });

  it("counts tools when it is working", () => {
    expect(
      stateOf(
        view({
          status: {
            name: "filesystem",
            description: "npx",
            connected: true,
            tool_count: 1,
            error: null,
            disabled: false,
            tools: ["read_file"],
          },
        }),
      ),
    ).toEqual({ tone: "ok", label: "1 tool" });
  });
});

describe("what the form refuses to save", () => {
  const draft = (patch: Partial<Parameters<typeof draftProblem>[0]> = {}) => ({
    name: "filesystem",
    scope: "global" as const,
    transport: "stdio" as const,
    command: "npx",
    args: [],
    env: [],
    url: "",
    headers: [],
    disabled: false,
    ...patch,
  });

  it("accepts a complete entry of either kind", () => {
    expect(draftProblem(draft())).toBeNull();
    expect(
      draftProblem(
        draft({ transport: "http", command: "", url: "https://e.com/mcp" }),
      ),
    ).toBeNull();
  });

  it("refuses a name that would break its own tool names", () => {
    // The name becomes part of every tool as `mcp__<server>__<tool>`, so these
    // produce tools the model cannot reliably call — and the symptom arrives
    // much later, as a server that connected fine and is never used.
    expect(draftProblem(draft({ name: "" }))).toMatch(/name/i);
    expect(draftProblem(draft({ name: "my server" }))).toMatch(/letters/i);
    expect(draftProblem(draft({ name: "a__b" }))).toMatch(/double underscore/i);
  });

  it("wants a way to reach the server", () => {
    expect(draftProblem(draft({ command: "  " }))).toMatch(/command/i);
    expect(
      draftProblem(draft({ transport: "http", command: "", url: "" })),
    ).toMatch(/URL/i);
    expect(
      draftProblem(
        draft({ transport: "http", command: "", url: "e.com/mcp" }),
      ),
    ).toMatch(/http/i);
  });

  it("lets a URL be entirely an environment variable", () => {
    // The documented way to keep an endpoint out of a committed file. Refusing
    // it for having no scheme would block the thing the syntax is for.
    expect(
      draftProblem(
        draft({ transport: "http", command: "", url: "${EXAMPLE_MCP_URL}" }),
      ),
    ).toBeNull();
  });

  it("catches two entries claiming the same name", () => {
    // They are written into an object, so the second silently wins and one of
    // the two rows on screen does nothing.
    expect(
      draftProblem(
        draft({
          env: [
            { key: "TOKEN", value: "a", secret: false },
            { key: "TOKEN", value: "b", secret: false },
          ],
        }),
      ),
    ).toMatch(/same name/i);
  });
});
