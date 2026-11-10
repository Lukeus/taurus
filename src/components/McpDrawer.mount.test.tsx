// @vitest-environment jsdom
//
// Mounted rather than rendered to a string, because the two things worth
// asserting about this drawer only exist once it is mounted: that it survives
// the store snapshot it selects from, and that the state of a server reaches the
// screen after the listing lands.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { McpDrawer } from "./McpDrawer";
import type { McpServerView } from "../lib/api";
import { useStore } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const server = (patch: Partial<McpServerView> = {}): McpServerView => ({
  name: "filesystem",
  scope: "global",
  transport: "stdio",
  command: "npx",
  args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
  env: [],
  url: "",
  headers: [],
  disabled: false,
  signed_in: false,
  program: "/opt/homebrew/bin/npx",
  ...patch,
});

const environment = {
  path: ["/opt/homebrew/bin", "/usr/bin"],
  source: "your login shell's PATH",
  added: ["/opt/homebrew/bin"],
};

/** Mounts the drawer and lets its initial load settle. */
const mount = async (servers: McpServerView[]) => {
  invoke.mockImplementation((command: string) => {
    if (command === "list_mcp_servers") return Promise.resolve(servers);
    if (command === "mcp_environment") return Promise.resolve(environment);
    return Promise.resolve(null);
  });

  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<McpDrawer onClose={() => {}} />);
  });
  return { host, unmount: () => act(() => root.unmount()) };
};

beforeEach(() => {
  invoke.mockReset();
  useStore.setState({ status: null });
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("opening the panel", () => {
  it("mounts and stays mounted before any status has arrived", async () => {
    // The bug this guards is the one the Skills drawer had: a selector that
    // ends in `?? []` or `.filter(…)` allocates a fresh array on every
    // snapshot, zustand v5 compares with `Object.is`, and React tears the tree
    // down rather than loop. The panel simply refuses to open.
    const { host, unmount } = await mount([]);
    expect(host.innerHTML).toContain("MCP servers");
    expect(host.innerHTML).toContain("No servers configured yet");
    unmount();
  });

  it("shows a connected server with what it offers", async () => {
    const { host, unmount } = await mount([
      server({
        status: {
          name: "filesystem",
          description: "npx",
          connected: true,
          tool_count: 11,
          error: null,
          disabled: false,
          tools: ["read_file", "write_file"],
          prompts: [],
        },
      }),
    ]);

    expect(host.innerHTML).toContain("filesystem");
    expect(host.innerHTML).toContain("dot ok");
    // In words as well as green.
    expect(host.querySelector(".tag.ok")?.textContent).toBe("connected");
    expect(host.innerHTML).toContain("Show 11 tools");
    expect(host.querySelector(".tools-toggle")?.getAttribute("aria-expanded")).toBe("false");
    // Where the program actually is, which is the fact a working server makes
    // uninteresting and a broken one makes everything.
    expect(host.innerHTML).toContain("/opt/homebrew/bin/npx");
    unmount();
  });

  it("says a missing program is missing, and where it looked", async () => {
    // The failure this whole panel was built for: the entry is correct, the
    // program is installed, and a window started from the Dock cannot see it.
    const { host, unmount } = await mount([
      server({
        program: undefined,
        status: {
          name: "filesystem",
          description: "npx",
          connected: false,
          tool_count: 0,
          error: "`npx` is not on this application's PATH",
          disabled: false,
          tools: [],
          prompts: [],
        },
      }),
    ]);

    const html = host.innerHTML;
    expect(html).toContain("dot error");
    expect(html).toContain("is not on Taurus");
    expect(html).toContain("PATH");
    // The search path opens itself for exactly this case rather than waiting to
    // be found.
    expect(html).toContain("/opt/homebrew/bin");
    expect(html).toContain("full path to the program");
    unmount();
  });

  it("keeps a disabled server on screen rather than hiding it", async () => {
    // Vanishing is indistinguishable from never having been configured, and the
    // only way to tell was to go and read the file.
    const { host, unmount } = await mount([
      server({ name: "github", disabled: true }),
    ]);
    expect(host.innerHTML).toContain("github");
    expect(host.innerHTML).toContain("Enable");
    unmount();
  });

  it("reports an entry that would not parse, which has no server to hang off", async () => {
    useStore.setState({
      status: {
        problems: [
          { source: "mcp", message: "mcp server 'typo' does not recognize `commnd`" },
          { source: "skills", message: "a skill problem, shown elsewhere" },
        ],
      } as never,
    });

    const { host, unmount } = await mount([]);
    expect(host.innerHTML).toContain("does not recognize");
    expect(host.innerHTML).not.toContain("shown elsewhere");
    unmount();
  });

  it("takes the listing a write returns rather than reading again after it", async () => {
    // A save that answered with the new state and was then followed by a
    // separate read could render the state from before it. Every write command
    // returns the listing for this reason.
    const { host, unmount } = await mount([server({ name: "before" })]);
    invoke.mockImplementation((command: string) => {
      if (command === "reload_mcp")
        return Promise.resolve([server({ name: "after" })]);
      return Promise.resolve(null);
    });

    const reconnect = [...host.querySelectorAll("button")].find(
      (b) => b.textContent === "Reconnect",
    );
    await act(async () => {
      reconnect?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(host.innerHTML).toContain("after");
    expect(host.innerHTML).not.toContain("before");
    unmount();
  });
});

describe("a server that wants an account", () => {
  /*
   * OAuth is offered on an HTTP server and never on a stdio one. That is not a
   * layout preference: the MCP authorization specification says a stdio server
   * takes its credentials from the environment, so a Sign in button there would
   * be a control for something that cannot happen.
   */
  const remote = (patch: Partial<McpServerView> = {}): McpServerView =>
    server({
      name: "linear",
      transport: "http",
      command: "",
      args: [],
      url: "https://mcp.linear.app/mcp",
      program: undefined,
      ...patch,
    });

  it("offers to sign in to an HTTP server", async () => {
    const { host } = await mount([remote()]);
    expect(host.textContent).toContain("Sign in");
  });

  it("offers to sign out of one already signed in, and says so", async () => {
    const { host } = await mount([remote({ signed_in: true })]);
    expect(host.textContent).toContain("Sign out");
    expect(host.textContent).toContain("signed in");
    expect(host.textContent).not.toContain("Sign in");
  });

  it("offers neither on a stdio server", async () => {
    const { host } = await mount([server()]);
    expect(host.textContent).not.toContain("Sign in");
    expect(host.textContent).not.toContain("Sign out");
  });
});

describe("what a server costs", () => {
  const connected = (patch: Partial<McpServerView> = {}) =>
    server({
      status: {
        name: "filesystem",
        description: "npx",
        connected: true,
        tool_count: 11,
        error: null,
        disabled: false,
        tools: ["read_file", "write_file"],
        prompts: [],
      },
      ...patch,
    });

  it("prices a connected server beside its tools, and the drawer as a whole", async () => {
    const { host } = await mount([connected({ schema_tokens: 1240 })]);
    // The per-server figure, and the total the header carries — the number the
    // switch beside it is actually for.
    expect(host.textContent).toContain("~1.2k tokens");
    expect(host.textContent).toContain("of every request");
  });

  it("says nothing at all about a server that is not connected", async () => {
    // Absence and zero are different answers here. A disabled server registers
    // no tools, so there is nothing to measure — and a `0` in this row would
    // read as "this one is free" rather than "not known".
    const { host } = await mount([server({ disabled: true })]);
    expect(host.textContent).not.toContain("tokens");
  });

  it("shows a real zero for a connected server that offers nothing", async () => {
    // Worth saying rather than hiding: a child process running for no benefit
    // is exactly what someone opening this panel is looking for. This is also
    // why the figure sits outside the tool list — inside it, the server with
    // the most to answer for would be the one that said nothing.
    const { host } = await mount([
      connected({
        schema_tokens: 0,
        status: {
          name: "filesystem",
          description: "npx",
          connected: true,
          tool_count: 0,
          error: null,
          disabled: false,
          tools: [],
          prompts: [],
        },
      }),
    ]);
    expect(host.textContent).toContain("~0 tokens of every request");
  });

  it("prices a switched-off server from the last time it ran, and says when", async () => {
    const twoHoursAgo = Math.floor(Date.now() / 1000) - 2 * 3600;
    const { host } = await mount([
      server({ disabled: true, last_seen_tokens: 3400, last_seen_at: twoHoursAgo }),
    ]);
    expect(host.textContent).toContain("~3.4k tokens of every request when on");
    expect(host.textContent).toContain("seen 2h ago");
  });

  it("says so when the project's own settings decide compact schemas", async () => {
    useStore.setState({
      status: {
        settings: { compact_mcp_schemas: false, from_project: ["compact_mcp_schemas"] },
        problems: [],
      } as never,
    });
    const { host } = await mount([connected({ schema_tokens: 1240 })]);
    expect(host.textContent).toContain(".taurus/settings.json sets this");
  });

  it("switching compact schemas on reprices the list it gets back", async () => {
    const { host } = await mount([connected({ schema_tokens: 1240 })]);
    invoke.mockImplementation((command: string) => {
      if (command === "set_compact_mcp_schemas")
        return Promise.resolve([connected({ schema_tokens: 400 })]);
      return Promise.resolve(null);
    });
    const box = [...host.querySelectorAll("label")]
      .find((label) => label.textContent?.includes("Compact schemas"))
      ?.querySelector("input");
    expect(box).toBeTruthy();
    await act(async () => {
      box!.click();
    });
    expect(invoke).toHaveBeenCalledWith("set_compact_mcp_schemas", { enabled: true });
    expect(host.textContent).toContain("~400 tokens of every request");
  });
});

describe("Reconnect", () => {
  it("reads the PATH again, and the search path shows what it found", async () => {
    // `npm i -g` in a terminal after launch: the program is there, and only a
    // fresh read of the PATH can see it.
    const { host, unmount } = await mount([]);
    const grown = {
      path: ["/Users/me/.npm-global/bin", "/opt/homebrew/bin", "/usr/bin"],
      source: "the PATH saved in your Windows settings",
      added: ["/opt/homebrew/bin", "/Users/me/.npm-global/bin"],
    };
    invoke.mockImplementation((command: string) => {
      if (command === "reload_mcp") return Promise.resolve([]);
      if (command === "mcp_environment") return Promise.resolve(grown);
      return Promise.resolve(null);
    });

    const reconnect = [...host.querySelectorAll("button")].find(
      (b) => b.textContent === "Reconnect",
    )!;
    await act(async () => reconnect.click());

    expect(invoke.mock.calls.map(([name]) => name)).toContain("reload_mcp");
    expect(host.innerHTML).toContain("3 directories");
    expect(host.innerHTML).toContain("added 2 directories");
    // Named for where it came from, which on Windows is not a shell.
    expect(host.textContent).toContain(
      "Taurus read the PATH saved in your Windows settings and added",
    );
    unmount();
  });
});
