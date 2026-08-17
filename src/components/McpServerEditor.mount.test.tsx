// @vitest-environment jsdom
//
// The editor is where a wrong answer costs something: a save that blanks a
// token nobody typed, or a rename that leaves two copies of one server behind.
// Both are only observable in what reaches the backend, so these assert on the
// payload rather than on the markup.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { McpServerEditor } from "./McpServerEditor";
import type { McpServerView } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const server = (patch: Partial<McpServerView> = {}): McpServerView => ({
  name: "github",
  scope: "global",
  transport: "stdio",
  command: "npx",
  args: ["-y", "@modelcontextprotocol/server-github"],
  env: [{ key: "GITHUB_TOKEN", value: "", secret: true }],
  url: "",
  headers: [],
  disabled: false,
  ...patch,
});

const mount = (existing: McpServerView | null) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() =>
    root.render(
      <McpServerEditor
        server={existing}
        onClose={() => {}}
        onSaved={() => {}}
      />,
    ),
  );
  return { host, unmount: () => act(() => root.unmount()) };
};

/** Presses the button whose visible text is `label`. */
const press = async (host: HTMLElement, label: string) => {
  const button = [...host.querySelectorAll("button")].find((b) =>
    b.textContent?.startsWith(label),
  );
  await act(async () => {
    button?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
};

/** Types into the input with `aria-label`, the way React sees it. */
const type = async (host: HTMLElement, label: string, value: string) => {
  const input = host.querySelector<HTMLInputElement>(`[aria-label="${label}"]`);
  // A selector that silently matched nothing would make every assertion below
  // it pass against a form nobody typed into.
  if (!input) throw new Error(`no input labelled ${label}`);
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set;
    setter?.call(input, value);
    input?.dispatchEvent(new Event("input", { bubbles: true }));
  });
};

const saved = () =>
  invoke.mock.calls.find(([command]) => command === "save_mcp_server")?.[1] as
    | {
        draft: Record<string, unknown>;
        previous?: { scope: string; name: string };
      }
    | undefined;

const tested = () =>
  invoke.mock.calls.find(([command]) => command === "test_mcp_server")?.[1] as
    | { previous?: { scope: string; name: string } }
    | undefined;

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue([]);
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("editing a server that holds a secret", () => {
  it("sends the secret back untouched, so a save does not blank it", () => {
    // The failure this exists for: the form was never given the token, so a
    // save of an unrelated field writes back the empty string it was shown and
    // the server starts failing to authenticate. The marker has to survive the
    // round trip for the backend to know to keep what it has.
    const { host, unmount } = mount(server());
    const value = host.querySelector<HTMLInputElement>(
      '[aria-label="Environment variables value 1"]',
    );
    expect(value?.type).toBe("password");
    expect(value?.value).toBe("");
    unmount();
  });

  it("marks a typed value as no longer held back", async () => {
    const { host, unmount } = mount(server());
    await type(host, "Environment variables value 1", "${GITHUB_TOKEN}");
    await press(host, "Save and reconnect");

    const env = saved()?.draft.env as { value: string; secret: boolean }[];
    expect(env[0]).toEqual({
      key: "GITHUB_TOKEN",
      value: "${GITHUB_TOKEN}",
      secret: false,
    });
    unmount();
  });
});

describe("saving", () => {
  it("splits the command line into a program and its arguments", async () => {
    const { host, unmount } = mount(null);
    await type(host, "Name", "filesystem");
    await type(
      host,
      "Command",
      'npx -y @modelcontextprotocol/server-filesystem "/Users/me/My Documents"',
    );
    await press(host, "Add and connect");

    expect(saved()?.draft.command).toBe("npx");
    expect(saved()?.draft.args).toEqual([
      "-y",
      "@modelcontextprotocol/server-filesystem",
      // Quoted, so it stays one argument — splitting it would start the server
      // in a directory that does not exist.
      "/Users/me/My Documents",
    ]);
    unmount();
  });

  it("always names the entry it is editing, so a secret is never lost", async () => {
    // Sent on every save rather than only on a rename. It is what the backend
    // reads the held-back token from, and sending it only when something had
    // changed meant a plain save could not find the secret it was meant to keep.
    // Whether the old entry is *removed* is then the backend's call, made by
    // comparing it with where the draft ended up.
    const unchanged = mount(server());
    await press(unchanged.host, "Save and reconnect");
    expect(saved()?.previous).toEqual({ scope: "global", name: "github" });
    unchanged.unmount();

    invoke.mockReset();
    invoke.mockResolvedValue([]);

    const renamed = mount(server());
    await type(renamed.host, "Name", "gh");
    await press(renamed.host, "Save and reconnect");
    expect(saved()?.draft.name).toBe("gh");
    expect(saved()?.previous).toEqual({ scope: "global", name: "github" });
    renamed.unmount();
  });

  it("sends no previous entry when adding, because there is none", async () => {
    const { host, unmount } = mount(null);
    await type(host, "Name", "filesystem");
    await type(host, "Command", "npx -y pkg");
    await press(host, "Add and connect");
    expect(saved()?.previous).toBeUndefined();
    unmount();
  });

  it("will not save an entry that cannot work, and says why", async () => {
    const { host, unmount } = mount(null);
    await type(host, "Name", "my server");

    const save = [...host.querySelectorAll("button")].find((b) =>
      b.textContent?.startsWith("Add and connect"),
    );
    expect(save?.disabled).toBe(true);
    // The reason sits beside the button rather than arriving after a round trip.
    expect(host.innerHTML).toContain("letters, digits, hyphens");
    unmount();
  });
});

describe("testing before saving", () => {
  it("connects the draft and writes nothing", async () => {
    invoke.mockImplementation((command: string) =>
      command === "test_mcp_server"
        ? Promise.resolve(["read_file", "write_file"])
        : Promise.resolve([]),
    );

    const { host, unmount } = mount(server());
    await press(host, "Test");

    expect(host.innerHTML).toContain("Connected");
    expect(host.innerHTML).toContain("2 tools");
    expect(
      invoke.mock.calls.some(([command]) => command === "save_mcp_server"),
    ).toBe(false);
    // Testing needs the stored credential as much as saving does, or checking an
    // entry whose token was not retyped would fail on the one thing the panel
    // deliberately never showed it.
    expect(tested()?.previous).toEqual({ scope: "global", name: "github" });
    unmount();
  });

  it("drops a result the moment the fields it came from change", async () => {
    // A green "2 tools" hanging above a field someone has since edited is the
    // one thing a Test button must never do.
    invoke.mockImplementation((command: string) =>
      command === "test_mcp_server" ? Promise.resolve(["read_file"]) : Promise.resolve([]),
    );

    const { host, unmount } = mount(server());
    await press(host, "Test");
    expect(host.innerHTML).toContain("Connected");

    await type(host, "Command", "npx -y something-else");
    expect(host.innerHTML).not.toContain("Connected");
    unmount();
  });

  it("shows the failure where the fields that caused it are", async () => {
    invoke.mockImplementation((command: string) =>
      command === "test_mcp_server"
        ? Promise.reject("`npx` is not on this application's PATH")
        : Promise.resolve([]),
    );

    const { host, unmount } = mount(server());
    await press(host, "Test");
    expect(host.innerHTML).toContain("test-result error");
    expect(host.innerHTML).toContain("PATH");
    unmount();
  });
});
