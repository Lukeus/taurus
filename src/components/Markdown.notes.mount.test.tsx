// @vitest-environment jsdom
//
// What Markdown does differently inside a note being read: its tasks can be
// ticked, its links to other notes open them, and a keystroke above a diagram
// does not lay the diagram out again. All three need a real render — a click,
// or two renders compared — which `Markdown.test.tsx`'s string renders cannot
// give.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const opened = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => {
    opened(...args);
    return Promise.resolve();
  },
}));

/** How many times a flowchart was laid out. */
let planned = 0;
vi.mock("../lib/flow", async (original) => {
  const actual = await original<typeof import("../lib/flow")>();
  return {
    ...actual,
    plan: (...args: Parameters<typeof actual.plan>) => {
      planned += 1;
      return actual.plan(...args);
    },
  };
});

const { Markdown, NoteHost } = await import("./Markdown");

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type Host = NonNullable<React.ContextType<typeof NoteHost>>;

let cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  planned = 0;
  opened.mockReset();
});

/** Renders `text`, in a note when `host` is given, and hands back a way to
 *  render it again with other text. */
function mount(text: string, host: Host | null, keyBy: "offset" | "content" = "content") {
  const box = document.createElement("div");
  document.body.appendChild(box);
  const root = createRoot(box);
  const render = (next: string) =>
    act(() =>
      root.render(
        <NoteHost.Provider value={host}>
          <Markdown text={next} streaming={false} keyBy={keyBy} />
        </NoteHost.Provider>,
      ),
    );
  render(text);
  cleanup.push(() => {
    act(() => root.unmount());
    box.remove();
  });
  return { box, render };
}

const noteHost = (over: Partial<Host> = {}): Host => ({
  hasNote: () => true,
  openNote: vi.fn(),
  toggleTask: vi.fn(),
  ...over,
});

describe("a task in a note", () => {
  // Three stretches, so the tasks are in one that does not start at 0 — which
  // is the case where a position counted from the stretch would be wrong.
  const TEXT = "# Plan\n\nWhat is left.\n\n## Tasks\n\n- [ ] ship it\n- [x] test it\n";

  it("hands over where its own item starts in the whole note", () => {
    const host = noteHost();
    const { box } = mount(TEXT, host);
    const boxes = box.querySelectorAll<HTMLInputElement>("input[type=checkbox]");
    expect([...boxes].map((b) => b.checked)).toEqual([false, true]);
    expect(boxes[1].disabled).toBe(false);
    act(() => boxes[1].click());
    expect(host.toggleTask).toHaveBeenCalledWith(TEXT.indexOf("- [x] test it"));
  });

  it("stays the parser's disabled box anywhere but a note", () => {
    // A model's reply with a task list in it has no notebook to tick it in.
    const { box } = mount(TEXT, null, "offset");
    const boxes = box.querySelectorAll<HTMLInputElement>("input[type=checkbox]");
    expect(boxes).toHaveLength(2);
    expect([...boxes].every((b) => b.disabled)).toBe(true);
  });
});

describe("a link in a note", () => {
  const TEXT = "See [the store](<Token store.md>), [the old plan](<Gone.md>) and [the RFC](https://example.com).\n";

  it("opens the note it names", () => {
    const host = noteHost({ hasNote: (name) => name === "Token store" });
    const { box } = mount(TEXT, host);
    const link = [...box.querySelectorAll("a")].find((a) => a.textContent === "the store")!;
    act(() => link.click());
    expect(host.openNote).toHaveBeenCalledWith("Token store");
    expect(opened).not.toHaveBeenCalled();
  });

  it("says a note that is not there is not there, and opens nothing", () => {
    const host = noteHost({ hasNote: (name) => name === "Token store" });
    const { box } = mount(TEXT, host);
    const link = [...box.querySelectorAll("a")].find((a) => a.textContent === "the old plan")!;
    expect(link.classList.contains("missing")).toBe(true);
    expect(link.getAttribute("data-tip")).toContain('no note called "Gone"');
    act(() => link.click());
    expect(host.openNote).not.toHaveBeenCalled();
    expect(opened).not.toHaveBeenCalled();
  });

  it("still sends a web address to the browser", () => {
    const { box } = mount(TEXT, noteHost());
    const link = [...box.querySelectorAll("a")].find((a) => a.textContent === "the RFC")!;
    act(() => link.click());
    expect(opened).toHaveBeenCalledWith("https://example.com");
  });
});

describe("a note being edited", () => {
  const DIAGRAM = "```mermaid\nflowchart LR\n  a[Client] --> b[API]\n```\n";

  it("does not lay a diagram out again for a keystroke above it", () => {
    const { render } = mount(`Intro.\n\n${DIAGRAM}`, noteHost());
    expect(planned).toBe(1);
    render(`Intro, edited.\n\n${DIAGRAM}`);
    expect(planned).toBe(1);
  });

  it("would, keyed by where each stretch starts — which is what the transcript keeps", () => {
    // The other half of the test above: proof that it measures something.
    const { render } = mount(`Intro.\n\n${DIAGRAM}`, null, "offset");
    expect(planned).toBe(1);
    render(`Intro, edited.\n\n${DIAGRAM}`);
    expect(planned).toBe(2);
  });
});
