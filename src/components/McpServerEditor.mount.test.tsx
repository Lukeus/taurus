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
  signed_in: false,
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

let keychain = true;

beforeEach(() => {
  invoke.mockReset();
  keychain = true;
  invoke.mockImplementation((command: string) =>
    Promise.resolve(command === "keychain_available" ? keychain : []),
  );
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

  it("won't add a second server under a name that file already has", async () => {
    // A save is an upsert by name, so "Add another" from the catalog, saved
    // without renaming, replaced the first server and took its token.
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    const render = (existing: McpServerView | null) =>
      act(() =>
        root.render(
          <McpServerEditor
            server={existing}
            taken={[server()]}
            onClose={() => {}}
            onSaved={() => {}}
          />,
        ),
      );
    render(null);
    await type(host, "Name", "github");
    await type(host, "Command", "npx -y pkg");
    const button = (label: string) =>
      [...host.querySelectorAll("button")].find((b) => b.textContent?.startsWith(label));
    expect(button("Add and connect")?.disabled).toBe(true);
    expect(host.textContent).toContain("A server called github is already in ~/.taurus/mcp.json");

    await type(host, "Name", "github-work");
    expect(button("Add and connect")?.disabled).toBe(false);
    act(() => root.unmount());

    // Editing that same server is not a clash with itself.
    const again = mount(server());
    expect(again.host.textContent).not.toContain("is already in");
    again.unmount();
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

describe("a token typed into the form", () => {
  const remote = server({
    name: "linear",
    transport: "http",
    command: "",
    args: [],
    env: [],
    url: "https://mcp.linear.app/mcp",
    headers: [{ key: "Authorization", value: "", secret: false }],
  });

  const box = (host: HTMLElement) =>
    host.querySelector<HTMLInputElement>('[aria-label="Keep headers value 1 in the keychain"]');

  it("is kept in the keychain unless unticked", async () => {
    const { host, unmount } = mount(remote);
    await act(async () => {});
    await type(host, "Headers value 1", "Bearer lin_123");
    expect(box(host)?.checked).toBe(true);
    await press(host, "Save and reconnect");
    const headers = saved()?.draft.headers as { value: string; keep?: boolean }[];
    expect(headers[0]).toMatchObject({ value: "Bearer lin_123", keep: true });
    unmount();
  });

  it("goes into the file when unticked", async () => {
    const { host, unmount } = mount(remote);
    await act(async () => {});
    await type(host, "Headers value 1", "Bearer lin_123");
    await act(async () => box(host)!.click());
    await press(host, "Save and reconnect");
    const headers = saved()?.draft.headers as { keep?: boolean }[];
    expect(headers[0].keep).toBe(false);
    unmount();
  });

  it("isn't offered on a value that only names one", async () => {
    const { host, unmount } = mount(remote);
    await act(async () => {});
    await type(host, "Headers value 1", "Bearer ${secret:linear}");
    expect(box(host)).toBeNull();
    await press(host, "Save and reconnect");
    const headers = saved()?.draft.headers as { keep?: boolean }[];
    expect(headers[0].keep).toBeUndefined();
    unmount();
  });

  it("isn't offered to a project's own program, and the hint says why", async () => {
    const local = server({
      scope: "workspace",
      env: [{ key: "GITHUB_TOKEN", value: "", secret: false }],
    });
    const { host, unmount } = mount(local);
    await act(async () => {});
    await type(host, "Environment variables value 1", "ghp_abc");
    expect(
      host.querySelector('[aria-label="Keep environment variables value 1 in the keychain"]'),
    ).toBeNull();
    expect(host.textContent).toContain("a project's own program can't use a saved credential");
    unmount();
  });

  it("isn't offered without a keychain", async () => {
    keychain = false;
    const { host, unmount } = mount(remote);
    await act(async () => {});
    await type(host, "Headers value 1", "Bearer lin_123");
    expect(box(host)).toBeNull();
    unmount();
  });

  it("leaves an environment variable that doesn't look like a token unticked", async () => {
    const { host, unmount } = mount(
      server({ env: [{ key: "LOG_LEVEL", value: "", secret: false }] }),
    );
    await act(async () => {});
    await type(host, "Environment variables value 1", "debug");
    const toggle = host.querySelector<HTMLInputElement>(
      '[aria-label="Keep environment variables value 1 in the keychain"]',
    );
    expect(toggle?.checked).toBe(false);
    unmount();
  });
});
