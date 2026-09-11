// @vitest-environment jsdom
//
// Everything worth checking about this pane happens after the first paint: the
// list arrives in an effect, opening a note is a second read, and a save is a
// debounce away from the last keystroke. A string render would see none of it.
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Notebook } from "../state/notebook";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
// The opener plugin reaches into Tauri internals that do not exist outside the
// webview. Only a link in the rendered Markdown would use it, and nothing here
// clicks one.
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

/*
 * Excalidraw cannot mount here — jsdom has no canvas — so the editor is a stand-in
 * that shows what it was given and has one button that "draws". What is under
 * test is everything around it: that a sketch is made, opened, saved and
 * reloaded by the same machinery as a note. The real canvas is photographed in
 * the `sketch` screenshot, which is its only check.
 */
const strokes = vi.hoisted(() => ({ count: 0, held: null as string | null }));
vi.mock("./SketchEditor", () => ({
  default: ({
    text,
    generation,
    onChange,
    drain,
  }: {
    text: string;
    generation: number;
    onChange: (text: string) => void;
    drain?: { current: (() => void) | null };
  }) => {
    // A stroke the real editor would still be holding on its own debounce,
    // handed over only when the notebook asks for it.
    if (drain) {
      drain.current = () => {
        if (strokes.held === null) return;
        onChange(strokes.held);
        strokes.held = null;
      };
    }
    return (
      <div className="stub-sketch" data-generation={generation}>
        <pre>{text}</pre>
        <button
          onClick={() =>
            onChange(`{"type":"excalidraw","elements":[{"id":"s${++strokes.count}"}]}`)
          }
        >
          draw
        </button>
        <button
          onClick={() => {
            strokes.held = `{"type":"excalidraw","elements":[{"id":"held${++strokes.count}"}]}`;
          }}
        >
          hold
        </button>
      </div>
    );
  },
}));
// And the picture an embed draws, for the same reason.
vi.mock("../lib/sketchSvg", () => ({
  draw: async () => {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("data-drawn", "yes");
    return svg;
  },
}));

const { NotesPane } = await import("./NotesPane");
const { useNotebook } = await import("../state/notebook");

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const ref = (
  name: string,
  scope: "workspace" | "global" = "workspace",
  kind: "note" | "sketch" = "note",
) => ({
  scope,
  kind,
  name,
  at: Math.floor(Date.now() / 1000) - 60,
  bytes: 32,
});

const page = (
  name: string,
  text: string,
  scope: "workspace" | "global" = "workspace",
  kind: "note" | "sketch" = "note",
) => ({
  scope,
  kind,
  name,
  text,
  fingerprint: "32-1000",
});

/**
 * The pane with the state it is a view over.
 *
 * `useNotebook` lives in `App` so that a half-typed paragraph survives a switch
 * to the transcript, so the two are only ever used together and testing them
 * apart would prove nothing about either.
 */
function Harness({
  hasWorkspace = true,
  onAsk = () => {},
  busy = false,
  workspace = "/code/taurus",
  errors,
  onNotebook,
}: {
  hasWorkspace?: boolean;
  onAsk?: (draft: string) => void;
  /** Whether a turn is running — moved from true to false to finish one. */
  busy?: boolean;
  /** The folder that is open — changed to switch workspace. */
  workspace?: string | null;
  /** Where the errors `App` would show end up. */
  errors?: string[];
  /** Hands out the notebook, for what `App` calls on it directly. */
  onNotebook?: (notebook: Notebook) => void;
}) {
  const [wrote] = useState<{ at: number; paths: string[] } | null>(null);
  const notebook = useNotebook({
    wrote,
    busy,
    workspace,
    onError: (message) => errors?.push(message),
  });
  onNotebook?.(notebook);
  return <NotesPane notebook={notebook} onAsk={onAsk} hasWorkspace={hasWorkspace} />;
}

let cleanup: (() => void)[] = [];

async function mount(props: Parameters<typeof Harness>[0] = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  await act(async () => {
    root.render(<Harness {...props} />);
  });
  return {
    host,
    /** Renders again with new props, the way `App` does when the store moves. */
    rerender: async (next: Parameters<typeof Harness>[0]) => {
      await act(async () => {
        root.render(<Harness {...props} {...next} />);
      });
    },
    click: async (element: Element | null | undefined) => {
      await act(async () => {
        (element as HTMLElement).click();
      });
    },
    /** Types into a controlled field the way React's own tracker will notice. */
    type: async (element: Element | null | undefined, value: string) => {
      const field = element as HTMLTextAreaElement | HTMLInputElement;
      const setter = Object.getOwnPropertyDescriptor(
        field instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : HTMLInputElement.prototype,
        "value",
      )!.set!;
      await act(async () => {
        setter.call(field, value);
        field.dispatchEvent(new Event("input", { bubbles: true }));
      });
    },
    press: async (element: Element | null | undefined, key: string) => {
      await act(async () => {
        (element as HTMLElement).dispatchEvent(
          new KeyboardEvent("keydown", { key, bubbles: true }),
        );
      });
    },
  };
}

const saying = (host: HTMLElement, label: string) =>
  [...host.querySelectorAll("button")].find((b) => b.textContent === label);

beforeEach(() => {
  invoke.mockReset();
});

afterEach(() => {
  // Unmounted, and not only emptied out of the page. A root left mounted
  // keeps the notebook's effects and whatever React has scheduled for them,
  // and on a loaded CI runner that work ran after the test environment was
  // torn down: `window is not defined` from React's scheduler, reported
  // against this file with every one of its tests passing.
  cleanup.forEach((fn) => fn());
  cleanup = [];
  document.body.innerHTML = "";
  vi.useRealTimers();
});

/** Answers one command, whatever order the pane asks in. */
function answering(handlers: Record<string, (args: never) => unknown>) {
  invoke.mockImplementation((name: string, args: never) => {
    const handler = handlers[name];
    if (!handler) return Promise.reject(`nothing mocked for ${name}`);
    const answer = handler(args);
    return answer instanceof Error ? Promise.reject(answer.message) : Promise.resolve(answer);
  });
}

describe("the notes list", () => {
  it("keeps the two notebooks apart and says which is which", async () => {
    answering({
      list_pages: () => [ref("Auth redesign"), ref("Reading list", "global")],
    });
    const { host } = await mount();

    expect(host.textContent).toContain("Project");
    expect(host.textContent).toContain("Global");
    const groups = [...host.querySelectorAll(".notes-group")];
    expect(groups[0].textContent).toContain("Auth redesign");
    expect(groups[0].textContent).not.toContain("Reading list");
    expect(groups[1].textContent).toContain("Reading list");
  });

  it("explains the two scopes before a note is chosen", async () => {
    // The one moment somebody is looking for the difference between them.
    answering({ list_pages: () => [] });
    const { host } = await mount();

    expect(host.textContent).toContain(".taurus/notes/");
    expect(host.textContent).toContain("committed with the repository");
    expect(host.textContent).toContain("~/.taurus/notes/");
  });

  it("says what to do about project notes with no folder open", async () => {
    // Said rather than left blank: an empty group with no sentence reads as
    // something broken rather than as something waiting.
    answering({ list_pages: () => [] });
    const { host } = await mount({ hasWorkspace: false });

    expect(host.textContent).toContain("Open a folder to keep notes with it");
    // And there is no way to start one that would only come back refused.
    expect(host.querySelectorAll(".notes-new")).toHaveLength(1);
  });

  it("refuses a name where the box that asked for it is", async () => {
    answering({
      list_pages: () => [],
      create_page: () => new Error("There is already a note called 'Notes'."),
    });
    const { host, click, type, press } = await mount();

    await click(host.querySelector(".notes-new"));
    const field = host.querySelector(".notes-make-name");
    await type(field, "Notes");
    await press(field, "Enter");

    expect(host.querySelector(".notes-make")?.textContent).toContain(
      "already a note called 'Notes'",
    );
    // The name is still in the box, so it can be edited rather than retyped.
    expect((field as HTMLInputElement).value).toBe("Notes");
  });
});

describe("a note that is open", () => {
  const openOne = async (text: string, scope: "workspace" | "global" = "workspace") => {
    answering({
      list_pages: () => [ref("Auth redesign", scope)],
      read_page: () => page("Auth redesign", text, scope),
      save_page: (args: never) => ({
        type: "written",
        page: { ...page("Auth redesign", (args as { text: string }).text, scope), fingerprint: "40-2000" },
      }),
    });
    const mounted = await mount();
    await mounted.click(mounted.host.querySelector(".notes-row"));
    return mounted;
  };

  it("reads it off disk rather than from the list", async () => {
    // The list carries no text, for the reason a dataset is a handle: a
    // remembered copy is wrong the moment anything else writes the file.
    const { host } = await openOne("# Auth redesign\n\nUse a refresh token.\n");

    expect(invoke).toHaveBeenCalledWith("read_page", {
      scope: "workspace",
      kind: "note",
      name: "Auth redesign",
    });
    expect(host.querySelector("textarea")?.value).toContain("Use a refresh token.");
  });

  it("says where the file is, because that is what says whether it is committed", async () => {
    const { host } = await openOne("# Auth redesign\n");
    expect(host.querySelector(".notes-where")?.textContent).toBe(
      ".taurus/notes/Auth redesign.md",
    );
  });

  it("renders the Markdown in Read, and draws a mermaid block", async () => {
    const { host, click } = await openOne(
      "# Auth redesign\n\n```mermaid\nflowchart LR\n  a[Client] --> b[API]\n```\n",
    );
    await click(saying(host, "Read"));

    expect(host.querySelector("h1")?.textContent).toBe("Auth redesign");
    // The whole point of the fence: a picture, in the app's own palette.
    expect(host.querySelector("svg.flow")).not.toBeNull();
    expect(host.textContent).toContain("Client");
  });

  it("shows what is typed rather than what is on disk", async () => {
    // A preview of the saved version while the editor holds a newer one would
    // be two answers to the same question on one screen.
    const { host, click, type } = await openOne("# Auth redesign\n");
    await type(host.querySelector("textarea"), "# Auth redesign\n\ntyped since\n");
    await click(saying(host, "Read"));

    expect(host.querySelector(".notes-read")?.textContent).toContain("typed since");
  });

  it("names the note in the sentence it hands the composer", async () => {
    // Names it rather than quoting it: `read_note` reads the file as it is now,
    // and a pasted copy in the transcript is wrong as soon as either writer
    // touches it.
    const drafts: string[] = [];
    answering({
      list_pages: () => [ref("Auth redesign")],
      read_page: () => page("Auth redesign", "# Auth redesign\n"),
    });
    const { host, click } = await mount({ onAsk: (d) => drafts.push(d) });
    await click(host.querySelector(".notes-row"));
    await click(saying(host, "Ask about this"));

    expect(drafts).toEqual(['About my project note "Auth redesign": ']);
  });

  it("needs two presses to delete one", async () => {
    answering({
      list_pages: () => [ref("Auth redesign")],
      read_page: () => page("Auth redesign", "# Auth redesign\n"),
      forget_page: () => [],
    });
    const { host, click } = await mount();
    await click(host.querySelector(".notes-row"));

    await click(saying(host, "Delete"));
    expect(invoke).not.toHaveBeenCalledWith("forget_page", expect.anything());
    expect(saying(host, "Delete it")).toBeDefined();

    await click(saying(host, "Delete it"));
    expect(invoke).toHaveBeenCalledWith("forget_page", {
      scope: "workspace",
      kind: "note",
      name: "Auth redesign",
    });
    // And the pane goes back to the list rather than showing a note that is gone.
    expect(host.querySelector(".notes-none")).not.toBeNull();
  });
});

describe("saving a note", () => {
  it("writes once typing stops, against the fingerprint it read", async () => {
    vi.useFakeTimers();
    answering({
      list_pages: () => [ref("Notes")],
      read_page: () => page("Notes", "# Notes\n"),
      save_page: () => ({
        type: "written",
        page: { ...page("Notes", "# Notes\n\nmore\n"), fingerprint: "40-2000" },
      }),
    });
    const { host, click, type } = await mount();
    await click(host.querySelector(".notes-row"));
    await type(host.querySelector("textarea"), "# Notes\n\nmore\n");

    // Nothing yet: a sentence is one save rather than forty.
    expect(invoke).not.toHaveBeenCalledWith("save_page", expect.anything());
    expect(host.querySelector(".notes-state")?.textContent).toBe("Unsaved");

    await act(async () => {
      vi.advanceTimersByTime(900);
    });

    expect(invoke).toHaveBeenCalledWith("save_page", {
      scope: "workspace",
      kind: "note",
      name: "Notes",
      text: "# Notes\n\nmore\n",
      fingerprint: "32-1000",
    });
  });

  it("keeps both versions when somebody got there first", async () => {
    // The rule this exists for: the one thing that must not happen is the app
    // deciding whose work to throw away.
    vi.useFakeTimers();
    answering({
      list_pages: () => [ref("Notes")],
      read_page: () => page("Notes", "# Notes\n"),
      save_page: () => ({
        type: "stale",
        current: { ...page("Notes", "# Notes\n\ntheirs\n"), fingerprint: "44-3000" },
      }),
    });
    const { host, click, type } = await mount();
    await click(host.querySelector(".notes-row"));
    await type(host.querySelector("textarea"), "# Notes\n\nmine\n");
    await act(async () => {
      vi.advanceTimersByTime(900);
    });

    const bar = host.querySelector(".notes-conflict");
    expect(bar).not.toBeNull();
    // Not "overwrote": nothing was, which is the entire point.
    expect(bar?.textContent).toContain("changed while you were working on it");
    // And what was typed is still in the editor.
    expect(host.querySelector("textarea")?.value).toContain("mine");
  });

  it("stops saving while the conflict is open, and resumes on a choice", async () => {
    // Repeating the refusal every 800ms would bury the question under its own
    // answer.
    vi.useFakeTimers();
    let saves = 0;
    answering({
      list_pages: () => [ref("Notes")],
      read_page: () => page("Notes", "# Notes\n"),
      save_page: () => {
        saves += 1;
        return saves === 1
          ? { type: "stale", current: { ...page("Notes", "theirs\n"), fingerprint: "44-3000" } }
          : { type: "written", page: { ...page("Notes", "mine\n"), fingerprint: "48-4000" } };
      },
    });
    const { host, click, type } = await mount();
    await click(host.querySelector(".notes-row"));
    await type(host.querySelector("textarea"), "mine\n");
    await act(async () => {
      vi.advanceTimersByTime(3000);
    });
    expect(saves).toBe(1);

    await click(saying(host, "Keep mine"));
    await act(async () => {
      vi.advanceTimersByTime(900);
    });

    expect(saves).toBe(2);
    // Written against the fingerprint the file actually has now, which is what
    // makes the second attempt land rather than be refused again. Not the *last*
    // call: a save that lands re-reads the list, because it changed the size and
    // the timestamp shown in it.
    expect(invoke).toHaveBeenCalledWith("save_page", {
      scope: "workspace",
      kind: "note",
      name: "Notes",
      text: "mine\n",
      fingerprint: "44-3000",
    });
  });

  it("takes the other version when that is the choice", async () => {
    vi.useFakeTimers();
    answering({
      list_pages: () => [ref("Notes")],
      read_page: () => page("Notes", "# Notes\n"),
      save_page: () => ({
        type: "stale",
        current: { ...page("Notes", "theirs\n"), fingerprint: "44-3000" },
      }),
    });
    const { host, click, type } = await mount();
    await click(host.querySelector(".notes-row"));
    await type(host.querySelector("textarea"), "mine\n");
    await act(async () => {
      vi.advanceTimersByTime(900);
    });
    await click(saying(host, "Take theirs"));

    expect(host.querySelector("textarea")?.value).toBe("theirs\n");
    expect(host.querySelector(".notes-conflict")).toBeNull();
  });
});

const EMPTY = '{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}}';

describe("a sketch", () => {
  it("is made instead of a note when that is chosen", async () => {
    answering({
      list_pages: () => [],
      create_page: (args: never) =>
        page((args as { name: string }).name, EMPTY, "workspace", "sketch"),
      read_page: (args: never) =>
        page((args as { name: string }).name, EMPTY, "workspace", "sketch"),
    });
    const { host, click, type, press } = await mount();

    await click(host.querySelector(".notes-new"));
    await click(saying(host, "Sketch"));
    const field = host.querySelector(".notes-make-name");
    await type(field, "Flow");
    await press(field, "Enter");

    expect(invoke).toHaveBeenCalledWith("create_page", {
      scope: "workspace",
      kind: "sketch",
      name: "Flow",
    });
    await vi.waitFor(() => expect(host.querySelector(".stub-sketch")).not.toBeNull());
    // Nothing in a sketch is Markdown, and nothing in it is something the model
    // could read — so neither control is offered. The line that embeds it is.
    expect(saying(host, "Write")).toBeUndefined();
    expect(saying(host, "Ask about this")).toBeUndefined();
    expect(host.querySelector(".notes-head")?.textContent).toContain("Copy embed");
    expect(host.querySelector(".notes-where")?.textContent).toBe(".taurus/notes/Flow.excalidraw");
  });

  it("is saved the way a note is, against the fingerprint it was read with", async () => {
    answering({
      list_pages: () => [ref("Flow", "workspace", "sketch")],
      read_page: () => page("Flow", EMPTY, "workspace", "sketch"),
      save_page: (args: never) => ({
        type: "written",
        page: {
          ...page("Flow", (args as { text: string }).text, "workspace", "sketch"),
          fingerprint: "60-2000",
        },
      }),
    });
    const { host, click } = await mount();
    await click(host.querySelector(".notes-row"));
    await vi.waitFor(() => expect(host.querySelector(".stub-sketch")).not.toBeNull());
    const before = host.querySelector(".stub-sketch")?.getAttribute("data-generation");

    vi.useFakeTimers();
    await click(saying(host, "draw"));
    await act(async () => {
      vi.advanceTimersByTime(900);
    });

    expect(invoke).toHaveBeenCalledWith(
      "save_page",
      expect.objectContaining({
        scope: "workspace",
        kind: "sketch",
        name: "Flow",
        fingerprint: "32-1000",
      }),
    );
    // And the canvas was not reloaded by its own save. Excalidraw holds the undo
    // history and the tool in hand; a save that reset them would make the editor
    // unusable in exactly the moment somebody was using it.
    expect(host.querySelector(".stub-sketch")?.getAttribute("data-generation")).toBe(before);
  });

  it("is read into the canvas again when the other version is taken", async () => {
    answering({
      list_pages: () => [ref("Flow", "workspace", "sketch")],
      read_page: () => page("Flow", EMPTY, "workspace", "sketch"),
      save_page: () => ({
        type: "stale",
        current: { ...page("Flow", '{"elements":["theirs"]}', "workspace", "sketch"), fingerprint: "70-3000" },
      }),
    });
    const { host, click } = await mount();
    await click(host.querySelector(".notes-row"));
    await vi.waitFor(() => expect(host.querySelector(".stub-sketch")).not.toBeNull());
    const before = Number(host.querySelector(".stub-sketch")?.getAttribute("data-generation"));

    vi.useFakeTimers();
    await click(saying(host, "draw"));
    await act(async () => {
      vi.advanceTimersByTime(900);
    });
    expect(host.querySelector(".notes-conflict")?.textContent).toContain(
      "This sketch changed while you were working on it",
    );
    await click(saying(host, "Take theirs"));

    const canvas = host.querySelector(".stub-sketch");
    expect(Number(canvas?.getAttribute("data-generation"))).toBeGreaterThan(before);
    expect(canvas?.textContent).toContain("theirs");
  });
});

describe("a sketch embedded in a note", () => {
  it("is drawn in Read, and opens from there", async () => {
    answering({
      list_pages: () => [ref("Auth")],
      read_page: (args: never) => {
        const asked = args as { kind: string; name: string };
        return asked.kind === "sketch"
          ? page(asked.name, EMPTY, "workspace", "sketch")
          : page("Auth", "# Auth\n\n![The flow](<Flow.excalidraw>)\n");
      },
    });
    const { host, click } = await mount();
    await click(host.querySelector(".notes-row"));
    await click(saying(host, "Read"));

    await vi.waitFor(() =>
      expect(host.querySelector(".sketch-embed-body [data-drawn]")).not.toBeNull(),
    );
    // Found beside the note, in the note's own notebook.
    expect(invoke).toHaveBeenCalledWith("read_page", {
      scope: "workspace",
      kind: "sketch",
      name: "Flow",
    });
    // The alt text is the caption, and the file's name is what opens.
    expect(host.querySelector(".sketch-embed-name")?.textContent).toBe("The flow");
    await click(saying(host, "open"));
    await vi.waitFor(() => expect(host.querySelector(".stub-sketch")).not.toBeNull());
  });

  it("says a sketch that is not there is not there, and offers nothing to open", async () => {
    answering({
      list_pages: () => [ref("Auth")],
      read_page: (args: never) =>
        (args as { kind: string }).kind === "sketch"
          ? new Error("There is no sketch called 'Gone' any more.")
          : page("Auth", "# Auth\n\n![Gone](<Gone.excalidraw>)\n"),
    });
    const { host, click } = await mount();
    await click(host.querySelector(".notes-row"));
    await click(saying(host, "Read"));

    await vi.waitFor(() =>
      expect(host.textContent).toContain("There is no sketch called 'Gone' in this notebook"),
    );
    expect(saying(host, "open")).toBeUndefined();
  });
});

describe("when a turn finishes", () => {
  it("lists what the turn made and re-reads a global note it changed", async () => {
    // A global note is outside the workspace, so the turn's `files_changed`
    // never names it — the end of the turn is the only moment to look again.
    let listed = [ref("Reading list", "global")];
    let version = page("Reading list", "# Reading list\n", "global");
    answering({ list_pages: () => listed, read_page: () => version });
    const { host, click, rerender } = await mount();
    await click(host.querySelector(".notes-row"));
    expect(host.querySelector("textarea")?.value).toBe("# Reading list\n");

    await rerender({ busy: true });
    listed = [ref("Made by the turn"), ref("Reading list", "global")];
    version = { ...page("Reading list", "# Reading list\n\nSICP\n", "global"), fingerprint: "48-2000" };
    await rerender({ busy: false });

    await vi.waitFor(() => expect(host.querySelector("textarea")?.value).toContain("SICP"));
    expect(host.textContent).toContain("Made by the turn");
  });
});

/** Two notes, each read as `# <name>`, each save answered as written. */
function twoNotes(extra: Record<string, (args: never) => unknown> = {}) {
  answering({
    list_pages: () => [ref("One"), ref("Two")],
    read_page: (args: never) => {
      const { name } = args as { name: string };
      return page(name, `# ${name}\n`);
    },
    save_page: (args: never) => {
      const { name, text } = args as { name: string; text: string };
      return { type: "written", page: { ...page(name, text), fingerprint: "40-2000" } };
    },
    ...extra,
  });
}

const rows = (host: HTMLElement) => [...host.querySelectorAll(".notes-row")];
const saves = () =>
  invoke.mock.calls.filter(([name]) => name === "save_page").map(([, args]) => args);

/**
 * `One` as a file somebody else can write mid-test — set `disk.one` — and `Two`
 * as one nobody does. A save against a stamp the file no longer has is refused,
 * the way the host refuses it.
 */
function contested() {
  const disk = { one: page("One", "# One\n") };
  answering({
    list_pages: () => [ref("One"), ref("Two")],
    read_page: (args: never) =>
      (args as { name: string }).name === "One" ? disk.one : page("Two", "# Two\n"),
    save_page: (args: never) => {
      const { text, fingerprint } = args as { text: string; fingerprint: string };
      if (fingerprint !== disk.one.fingerprint) return { type: "stale", current: disk.one };
      disk.one = { ...page("One", text), fingerprint: `${text.length}-9000` };
      return { type: "written", page: disk.one };
    },
  });
  return disk;
}

describe("leaving a file", () => {
  it("writes what was typed before the next one opens, rather than dropping it", async () => {
    // The debounce is right while somebody types and wrong the moment they
    // leave. Cancelled by the switch, it took the last sentence with it.
    twoNotes();
    const { host, click, type } = await mount();
    await click(rows(host)[0]);
    await type(host.querySelector("textarea"), "# One\n\nthe last sentence\n");
    await click(rows(host)[1]);

    expect(invoke).toHaveBeenCalledWith("save_page", {
      scope: "workspace",
      kind: "note",
      name: "One",
      text: "# One\n\nthe last sentence\n",
      fingerprint: "32-1000",
    });
    await vi.waitFor(() => expect(host.querySelector("textarea")?.value).toBe("# Two\n"));
  });

  it("does not let a save answered late make the note that was left current again", async () => {
    // Applied, the late answer put `One` back under `Two`'s text, and the next
    // autosave wrote `Two` into `One`.
    let land: () => void = () => {};
    twoNotes({
      save_page: (args: never) => {
        const { name, text } = args as { name: string; text: string };
        const written = { type: "written", page: { ...page(name, text), fingerprint: "40-2000" } };
        return name === "One" ? new Promise((resolve) => (land = () => resolve(written))) : written;
      },
    });
    const { host, click, type } = await mount();
    await click(rows(host)[0]);
    await type(host.querySelector("textarea"), "# One\n\nmine\n");

    vi.useFakeTimers();
    await act(async () => {
      vi.advanceTimersByTime(900);
    });
    // In flight, and not sent a second time by the switch that follows.
    await click(rows(host)[1]);
    expect(saves().filter((s) => (s as { name: string }).name === "One")).toHaveLength(1);
    expect(host.querySelector("textarea")?.value).toBe("# Two\n");

    await act(async () => land());
    await act(async () => {
      vi.advanceTimersByTime(2000);
    });

    expect(host.querySelector(".notes-name")?.textContent).toBe("Two");
    expect(host.querySelector("textarea")?.value).toBe("# Two\n");
    expect(saves()).not.toContainEqual(expect.objectContaining({ name: "One", text: "# Two\n" }));
  });

  it("keeps yours when its save on the way out is refused, and asks when it is opened", async () => {
    // There is no editor left to ask in, so it is kept, marked and said — not
    // put up as a conflict over the note that is open now.
    const disk = contested();
    const errors: string[] = [];
    const { host, click, type } = await mount({ errors });
    await click(rows(host)[0]);
    disk.one = { ...page("One", "theirs\n"), fingerprint: "44-3000" };
    await type(host.querySelector("textarea"), "mine\n");
    await click(rows(host)[1]);

    await vi.waitFor(() => expect(rows(host)[0].textContent).toContain("two versions"));
    expect(errors.join()).toContain('"One" changed on disk');
    expect(host.querySelector("textarea")?.value).toBe("# Two\n");
    expect(host.querySelector(".notes-conflict")).toBeNull();

    await click(rows(host)[0]);
    expect(host.querySelector(".notes-conflict")).not.toBeNull();
    expect(host.querySelector("textarea")?.value).toBe("mine\n");
  });

  it("hands over a sketch's last stroke before leaving it", async () => {
    // The canvas waits 200ms before it serialises, on top of the save's own
    // debounce; a stroke finished just before a switch was in neither.
    answering({
      list_pages: () => [ref("Flow", "workspace", "sketch"), ref("Notes")],
      read_page: (args: never) =>
        (args as { kind: string }).kind === "sketch"
          ? page("Flow", EMPTY, "workspace", "sketch")
          : page("Notes", "# Notes\n"),
      save_page: (args: never) => {
        const { name, text, kind } = args as { name: string; text: string; kind: "note" | "sketch" };
        return { type: "written", page: { ...page(name, text, "workspace", kind), fingerprint: "60-2000" } };
      },
    });
    const { host, click } = await mount();
    await click(rows(host)[0]);
    await vi.waitFor(() => expect(host.querySelector(".stub-sketch")).not.toBeNull());
    await click(saying(host, "hold"));
    await click(rows(host)[1]);

    expect(invoke).toHaveBeenCalledWith(
      "save_page",
      expect.objectContaining({ kind: "sketch", name: "Flow", text: expect.stringContaining('"held') }),
    );
  });

  it("writes a note under its old name before renaming it", async () => {
    twoNotes({
      rename_page: (args: never) => {
        const { to } = args as { to: string };
        return page(to, "# One\n\nmine\n");
      },
    });
    const { host, click, type, press } = await mount();
    await click(rows(host)[0]);
    await type(host.querySelector("textarea"), "# One\n\nmine\n");
    await click(saying(host, "Rename"));
    const field = host.querySelector(".notes-rename");
    await type(field, "Renamed");
    await press(field, "Enter");

    const order = invoke.mock.calls
      .map(([name]) => name)
      .filter((name) => name === "save_page" || name === "rename_page");
    expect(order).toEqual(["save_page", "rename_page"]);
    expect(saves()[0]).toMatchObject({ name: "One", text: "# One\n\nmine\n" });
  });

  it("refuses to rename a note while its conflict is open", async () => {
    // Renamed, it would be read afresh under the new name and the version still
    // waiting to be chosen would go with the old editor.
    vi.useFakeTimers();
    twoNotes({
      save_page: () => ({
        type: "stale",
        current: { ...page("One", "theirs\n"), fingerprint: "44-3000" },
      }),
      rename_page: () => page("Renamed", "theirs\n"),
    });
    const { host, click, type, press } = await mount();
    await click(rows(host)[0]);
    await type(host.querySelector("textarea"), "mine\n");
    await act(async () => {
      vi.advanceTimersByTime(900);
    });
    expect(host.querySelector(".notes-conflict")).not.toBeNull();

    await click(saying(host, "Rename"));
    const field = host.querySelector(".notes-rename");
    await type(field, "Renamed");
    await press(field, "Enter");

    expect(invoke).not.toHaveBeenCalledWith("rename_page", expect.anything());
    expect(host.querySelector(".notes-doc")?.textContent).toContain("Choose a version first");
    expect(host.querySelector("textarea")?.value).toBe("mine\n");
  });
});

describe("switching workspace", () => {
  it("closes a project note and reads the list again", async () => {
    // Kept open, its next autosave would have landed in the new folder's file
    // of the same name.
    answering({ list_pages: () => [ref("Notes")], read_page: () => page("Notes", "# Notes\n") });
    const { host, click, rerender } = await mount();
    await click(host.querySelector(".notes-row"));
    expect(host.querySelector("textarea")).not.toBeNull();
    const listed = invoke.mock.calls.filter(([name]) => name === "list_pages").length;

    await rerender({ workspace: "/code/elsewhere" });

    expect(host.querySelector(".notes-none")).not.toBeNull();
    expect(invoke.mock.calls.filter(([name]) => name === "list_pages")).toHaveLength(listed + 1);
  });

  it("leaves a global note open, since it belongs to neither folder", async () => {
    answering({
      list_pages: () => [ref("Reading list", "global")],
      read_page: () => page("Reading list", "# Reading list\n", "global"),
    });
    const { host, click, rerender } = await mount();
    await click(host.querySelector(".notes-row"));
    await rerender({ workspace: "/code/elsewhere" });

    expect(host.querySelector("textarea")?.value).toBe("# Reading list\n");
  });
});

describe("where a refusal is shown", () => {
  it("keeps a refused create in the box that asked, not under the open note", async () => {
    twoNotes({ create_page: () => new Error("There is already a note called 'One'.") });
    const { host, click, type, press } = await mount();
    await click(rows(host)[0]);
    await click(host.querySelector(".notes-new"));
    const field = host.querySelector(".notes-make-name");
    await type(field, "One");
    await press(field, "Enter");

    expect(host.querySelector(".notes-make")?.textContent).toContain("already a note called 'One'");
    expect(host.querySelector(".notes-doc")?.textContent).not.toContain("already a note called");
  });

  it("does not carry a refused rename onto the next note", async () => {
    twoNotes({ rename_page: () => new Error("There is already a note called 'Two'.") });
    const { host, click, type, press } = await mount();
    await click(rows(host)[0]);
    await click(saying(host, "Rename"));
    const field = host.querySelector(".notes-rename");
    await type(field, "Two");
    await press(field, "Enter");
    await vi.waitFor(() =>
      expect(host.querySelector(".notes-doc")?.textContent).toContain("already a note called 'Two'"),
    );

    await click(rows(host)[1]);
    await vi.waitFor(() => expect(host.querySelector("textarea")?.value).toBe("# Two\n"));
    expect(host.textContent).not.toContain("already a note called");
  });
});

describe("a version kept for a note that was left", () => {
  it("keeps yours when a note is left mid-conflict, and asks again when it is opened", async () => {
    // Leaving is not a choice between the two versions, so it must not make one.
    vi.useFakeTimers();
    const disk = contested();
    const { host, click, type } = await mount();
    await click(rows(host)[0]);
    disk.one = { ...page("One", "theirs\n"), fingerprint: "44-3000" };
    await type(host.querySelector("textarea"), "mine\n");
    await act(async () => {
      vi.advanceTimersByTime(900);
    });
    expect(host.querySelector(".notes-conflict")).not.toBeNull();

    await click(rows(host)[1]);
    expect(host.querySelector("textarea")?.value).toBe("# Two\n");
    expect(rows(host)[0].textContent).toContain("two versions");

    await click(rows(host)[0]);
    expect(host.querySelector(".notes-conflict")).not.toBeNull();
    expect(host.querySelector("textarea")?.value).toBe("mine\n");
    // Back in the editor, so no longer marked in the list.
    expect(rows(host)[0].textContent).not.toContain("two versions");

    await click(saying(host, "Keep mine"));
    await act(async () => {
      vi.advanceTimersByTime(900);
    });
    expect(disk.one.text).toBe("mine\n");
  });

  it("keeps yours when its save on the way out fails, and saves it when it is opened", async () => {
    let failing = true;
    twoNotes({
      save_page: (args: never) => {
        if (failing) return new Error("The disk is full.");
        const { name, text } = args as { name: string; text: string };
        return { type: "written", page: { ...page(name, text), fingerprint: "40-2000" } };
      },
    });
    const errors: string[] = [];
    const { host, click, type } = await mount({ errors });
    await click(rows(host)[0]);
    await type(host.querySelector("textarea"), "# One\n\nmine\n");
    await click(rows(host)[1]);
    await vi.waitFor(() => expect(rows(host)[0].textContent).toContain("not saved"));
    expect(errors.join()).toContain("The disk is full.");

    failing = false;
    vi.useFakeTimers();
    await click(rows(host)[0]);
    // Nothing else has written it, so there is nothing to ask: yours saves.
    expect(host.querySelector(".notes-conflict")).toBeNull();
    expect(host.querySelector("textarea")?.value).toBe("# One\n\nmine\n");
    await act(async () => {
      vi.advanceTimersByTime(900);
    });
    const all = saves();
    expect(all[all.length - 1]).toMatchObject({
      name: "One",
      text: "# One\n\nmine\n",
      fingerprint: "32-1000",
    });
  });

  it("keeps yours when the save before a switch is refused, rather than dropping it", async () => {
    // The flush `App` runs before a switch can itself come back refused:
    // somebody wrote the file after this editor read it. Nothing on screen
    // holds the text once the folder changes, so it has to be kept like any
    // other version left behind.
    vi.useFakeTimers();
    const disk = contested();
    const errors: string[] = [];
    const handle: { notebook?: Notebook } = {};
    const { host, click, type, rerender } = await mount({
      errors,
      onNotebook: (notebook) => (handle.notebook = notebook),
    });
    await click(rows(host)[0]);
    disk.one = { ...page("One", "theirs\n"), fingerprint: "44-3000" };
    await type(host.querySelector("textarea"), "mine\n");

    // Straight to the switch, before the autosave has had its turn.
    await act(async () => {
      await handle.notebook!.flush(true);
    });
    await rerender({ workspace: "/code/elsewhere" });
    expect(errors.join()).toContain("is kept for when taurus is open again");

    await rerender({ workspace: "/code/taurus" });
    await click(rows(host)[0]);
    expect(host.querySelector(".notes-conflict")).not.toBeNull();
    expect(host.querySelector("textarea")?.value).toBe("mine\n");
  });

  it("keeps yours when the save before a switch fails", async () => {
    twoNotes({ save_page: () => new Error("The disk is full.") });
    const errors: string[] = [];
    const handle: { notebook?: Notebook } = {};
    const { host, click, type, rerender } = await mount({
      errors,
      onNotebook: (notebook) => (handle.notebook = notebook),
    });
    await click(rows(host)[0]);
    await type(host.querySelector("textarea"), "# One\n\nmine\n");

    await act(async () => {
      await handle.notebook!.flush(true);
    });
    await rerender({ workspace: "/code/elsewhere" });
    expect(errors.join()).toContain("The disk is full.");
    expect(errors.join()).toContain("is kept for when taurus is open again");

    await rerender({ workspace: "/code/taurus" });
    await click(rows(host)[0]);
    expect(host.querySelector("textarea")?.value).toBe("# One\n\nmine\n");
  });

  it("keeps a project note's version with its folder, across a switch and back", async () => {
    vi.useFakeTimers();
    const disk = contested();
    const errors: string[] = [];
    const handle: { notebook?: Notebook } = {};
    const { host, click, type, rerender } = await mount({
      errors,
      onNotebook: (notebook) => (handle.notebook = notebook),
    });
    await click(rows(host)[0]);
    disk.one = { ...page("One", "theirs\n"), fingerprint: "44-3000" };
    await type(host.querySelector("textarea"), "mine\n");
    await act(async () => {
      vi.advanceTimersByTime(900);
    });

    // What `App` does before it asks for the switch.
    await act(async () => {
      await handle.notebook!.flush(true);
    });
    await rerender({ workspace: "/code/elsewhere" });
    expect(host.querySelector(".notes-none")).not.toBeNull();
    expect(errors.join()).toContain("is kept for when taurus is open again");

    await rerender({ workspace: "/code/taurus" });
    await click(rows(host)[0]);
    expect(host.querySelector(".notes-conflict")).not.toBeNull();
    expect(host.querySelector("textarea")?.value).toBe("mine\n");
  });
});
