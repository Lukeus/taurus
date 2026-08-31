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
        },
      }),
    ]);

    expect(host.innerHTML).toContain("filesystem");
    expect(host.innerHTML).toContain("dot ok");
    expect(host.innerHTML).toContain("Show 11 tools");
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
          { source: "mcp", message: "mcp server 'typo' does not recognise `commnd`" },
          { source: "skills", message: "a skill problem, shown elsewhere" },
        ],
      } as never,
    });

    const { host, unmount } = await mount([]);
    expect(host.innerHTML).toContain("does not recognise");
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
