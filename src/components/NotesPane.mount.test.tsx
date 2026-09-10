// @vitest-environment jsdom
//
// Everything worth checking about this pane happens after the first paint: the
// list arrives in an effect, opening a note is a second read, and a save is a
// debounce away from the last keystroke. A string render would see none of it.
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
// The opener plugin reaches into Tauri internals that do not exist outside the
// webview. Only a link in the rendered Markdown would use it, and nothing here
// clicks one.
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const { NotesPane } = await import("./NotesPane");
const { useNotebook } = await import("../state/notebook");

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const ref = (name: string, scope: "workspace" | "global" = "workspace") => ({
  scope,
  name,
  at: Math.floor(Date.now() / 1000) - 60,
  bytes: 32,
});

const page = (name: string, text: string, scope: "workspace" | "global" = "workspace") => ({
  scope,
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
}: {
  hasWorkspace?: boolean;
  onAsk?: (draft: string) => void;
}) {
  const [wrote] = useState<{ at: number; paths: string[] } | null>(null);
  const notebook = useNotebook({ wrote, onError: () => {} });
  return <NotesPane notebook={notebook} onAsk={onAsk} hasWorkspace={hasWorkspace} />;
}

async function mount(props: Parameters<typeof Harness>[0] = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<Harness {...props} />);
  });
  return {
    host,
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
    expect(bar?.textContent).toContain("changed while you were typing");
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
