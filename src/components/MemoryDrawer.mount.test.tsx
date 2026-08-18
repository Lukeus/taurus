// @vitest-environment jsdom
//
// The drawer loads its notes in an effect and rewrites its list from what the
// backend answers a deletion with, so both of the things worth checking here
// happen after the first paint. A string render would see neither.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { MemoryDrawer } from "./MemoryDrawer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const note = (id: string, text: string) => ({
  id,
  at: Math.floor(Date.now() / 1000) - 3600,
  session: `session-${id}`,
  text,
});

let cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  invoke.mockReset();
});

async function mount(props: Parameters<typeof MemoryDrawer>[0]) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<MemoryDrawer {...props} />);
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return {
    host,
    click: async (element: Element | null | undefined) => {
      await act(async () => {
        (element as HTMLElement).click();
      });
    },
  };
}

const buttonSaying = (host: HTMLElement, label: string) =>
  [...host.querySelectorAll("button")].find((b) => b.textContent === label);

describe("the memory drawer", () => {
  it("shows what earlier conversations left behind", async () => {
    invoke.mockResolvedValue([note("a", "the migration is staged, not live")]);

    const { host } = await mount({ onClose: () => {} });

    expect(host.textContent).toContain("the migration is staged, not live");
    expect(invoke).toHaveBeenCalledWith("list_notes");
  });

  it("says there is nothing here in a way that needs no action", async () => {
    // An empty memory is the ordinary state of a new workspace, not a problem
    // to fix — and the user cannot write one of these themselves anyway.
    invoke.mockResolvedValue([]);

    const { host } = await mount({ onClose: () => {} });

    expect(host.textContent).toContain("Nothing has been written down");
    expect(host.textContent).toContain("the agent writes these itself");
  });

  it("redraws from what the backend says is left, not from its own guess", async () => {
    // The file is shared with a running turn, which may have appended to it
    // since this drawer opened. Trusting a local splice would show a list that
    // disagrees with what the next conversation is actually told.
    invoke.mockResolvedValueOnce([note("a", "first"), note("b", "second")]);
    const { host, click } = await mount({ onClose: () => {} });

    invoke.mockResolvedValueOnce([note("b", "second"), note("c", "written since")]);
    await click(buttonSaying(host, "Forget"));

    expect(invoke).toHaveBeenLastCalledWith("forget_note", { id: "a" });
    expect(host.textContent).not.toContain("first");
    expect(host.textContent).toContain("written since");
  });

  it("reports a failure to forget rather than pretending it worked", async () => {
    invoke.mockResolvedValueOnce([note("a", "first")]);
    const { host, click } = await mount({ onClose: () => {} });

    invoke.mockRejectedValueOnce("the notes file is read-only");
    await click(buttonSaying(host, "Forget"));

    expect(host.textContent).toContain("read-only");
    expect(host.textContent).toContain("first");
  });

  it("offers the conversation a note came from only where one can be opened", async () => {
    invoke.mockResolvedValue([note("a", "something")]);

    const { host } = await mount({ onClose: () => {} });
    expect(buttonSaying(host, "where it came from")).toBeUndefined();

    const opened: string[] = [];
    const withOpen = await mount({
      onClose: () => {},
      onOpenSession: (id: string) => opened.push(id),
    });
    await withOpen.click(buttonSaying(withOpen.host, "where it came from"));
    expect(opened).toEqual(["session-a"]);
  });
});
