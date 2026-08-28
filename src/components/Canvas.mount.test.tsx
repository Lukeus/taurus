// @vitest-environment jsdom
//
// The canvas and the editor inside it, wired up.
//
// What jsdom cannot check is anything measured: it reports every
// `getBoundingClientRect`, `clientHeight` and computed `line-height` as zero,
// so the virtualized window here always resolves to "all of it" and where a
// revealed line *lands* is unknowable. That is deliberate rather than a gap —
// `DocumentEditor` falls back to painting everything when it cannot measure a
// row, so these tests exercise the same code the app runs, minus the
// optimisation. Where the panel actually sits is a screenshot's job; see
// `docs/development.md`.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Canvas, draftFor } from "./Canvas";
import { DocumentEditor } from "./DocumentEditor";
import type { Document } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

const CODE = `fn main() {
    // three lines in
    println!("hello");
}
`;

const PROSE = `# Title

Some **bold** text.
`;

function doc(path: string, text: string): Document {
  return {
    path,
    text,
    lines: text.split("\n").length,
    fingerprint: "1-2",
  };
}

function mount(node: React.ReactNode) {
  const host = window.document.createElement("div");
  window.document.body.appendChild(host);
  const root = createRoot(host);
  act(() => {
    root.render(node);
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return host;
}

const noop = () => {};

function canvas(over: Partial<Parameters<typeof Canvas>[0]> = {}) {
  return mount(
    <Canvas
      path="src/main.rs"
      reveal={null}
      document={doc("src/main.rs", CODE)}
      draft={over.draft ?? over.document?.text ?? CODE}
      state="idle"
      flash={null}
      conflict={null}
      error={null}
      onEdit={noop}
      onKeepMine={noop}
      onTakeTheirs={noop}
      onSelect={noop}
      onAsk={noop}
      onClose={noop}
      {...over}
    />,
  );
}

const box = (host: HTMLElement) =>
  host.querySelector(".doc-input") as HTMLTextAreaElement;

/**
 * Selects a range and tells React, the way a drag does.
 *
 * Through `mouseup` rather than a `select` event, and that is not a
 * workaround: React implements `onSelect` on a plugin that listens to
 * `selectionchange` on the document, which jsdom does not fire. The editor
 * listens to `mouseup` and `keyup` for exactly this reason — the two things a
 * person actually does to finish a selection — so this drives the same handler
 * the app does.
 */
function select(host: HTMLElement, from: number, to: number) {
  const area = box(host);
  act(() => {
    area.setSelectionRange(from, to);
    area.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
  });
}

describe("the canvas", () => {
  it("draws nothing at all when no file is open", () => {
    const host = mount(
      <Canvas
        path={null}
        reveal={null}
        document={null}
        draft=""
        state="idle"
        flash={null}
        conflict={null}
        error={null}
        onEdit={noop}
        onKeepMine={noop}
        onTakeTheirs={noop}
        onSelect={noop}
        onAsk={noop}
        onClose={noop}
      />,
    );
    expect(host.querySelector(".canvas")).toBeNull();
  });

  /** The filename is what anybody is looking for; the folder only tells two of
   *  them apart. */
  it("names the file and shows its folder separately", () => {
    const host = canvas({ path: "crates/taurus-data/src/df.rs" });
    expect(host.querySelector(".canvas-name")?.textContent).toBe("df.rs");
    expect(host.querySelector(".canvas-where")?.textContent).toBe(
      "crates/taurus-data/src",
    );
  });

  it("says how long the file is", () => {
    expect(canvas().querySelector(".canvas-count")?.textContent).toBe("5 lines");
  });

  /** A read that failed says so instead of showing an empty editor, which
   *  would read as an empty file. */
  it("shows the problem rather than a blank document", () => {
    const host = canvas({ document: null, error: "src/main.rs is not a text file." });
    expect(host.querySelector(".canvas-problem")?.textContent).toContain(
      "not a text file",
    );
    expect(host.querySelector(".doc-input")).toBeNull();
  });

  /* ------------------------------------------------------------ preview */

  /** What somebody asked to see when they said "open the readme" is the
   *  readme, not its asterisks. */
  it("opens Markdown on its preview and code on its source", () => {
    const prose = canvas({
      path: "README.md",
      document: doc("README.md", PROSE),
    });
    expect(prose.querySelector(".canvas-preview")).not.toBeNull();
    expect(prose.querySelector(".doc-input")).toBeNull();

    expect(canvas().querySelector(".doc-input")).not.toBeNull();
    expect(canvas().querySelector(".canvas-preview")).toBeNull();
  });

  /** A control offering one choice is furniture. */
  it("offers the mode switch only where there are two modes", () => {
    expect(canvas().querySelector(".canvas-modes")).toBeNull();
    expect(
      canvas({ path: "README.md", document: doc("README.md", PROSE) }).querySelector(
        ".canvas-modes",
      ),
    ).not.toBeNull();
  });

  /**
   * Found by photographing it. A model that says "the rule is at lines 15–19"
   * and is answered with a rendered page where nothing is marked has had its
   * whole sentence thrown away — preview has nowhere to put a selection.
   */
  it("opens Markdown on its source when the call pointed at lines", () => {
    const host = canvas({
      path: "README.md",
      document: doc("README.md", PROSE),
      reveal: { from: 3, to: 3 },
    });
    expect(host.querySelector(".doc-input")).not.toBeNull();
    expect(host.querySelector(".canvas-preview")).toBeNull();
  });

  it("switches Markdown to its source and back", () => {
    const host = canvas({ path: "README.md", document: doc("README.md", PROSE) });
    const source = [...host.querySelectorAll("button")].find(
      (b) => b.textContent === "Source",
    )!;
    act(() => source.click());
    expect(host.querySelector(".doc-input")).not.toBeNull();
    expect(host.querySelector(".canvas-preview")).toBeNull();
  });

  /* ------------------------------------------------------- ask about this */

  it("offers nothing while nothing is selected", () => {
    expect(canvas().querySelector(".canvas-ask")).toBeNull();
  });

  it("offers to ask about a selection, and says which lines", () => {
    const host = canvas();
    // "// three lines in" is on line 2.
    select(host, CODE.indexOf("//"), CODE.indexOf("//") + 17);
    expect(host.querySelector(".canvas-ask-where")?.textContent).toBe("Line 2");
  });

  it("reports the selection up rather than keeping it", () => {
    const onSelect = vi.fn();
    const host = canvas({ onSelect });
    select(host, 0, 11);
    expect(onSelect).toHaveBeenCalledWith({
      from: 1,
      to: 1,
      text: "fn main() {",
    });
  });

  /** An empty selection is a click, and a bar that appeared on every click
   *  would be in the way of reading. */
  it("clears the offer when the selection collapses", () => {
    const onSelect = vi.fn();
    const host = canvas({ onSelect });
    select(host, 0, 11);
    expect(host.querySelector(".canvas-ask")).not.toBeNull();
    select(host, 4, 4);
    expect(host.querySelector(".canvas-ask")).toBeNull();
    expect(onSelect).toHaveBeenLastCalledWith(null);
  });

  /** The rule every draft in the app follows: fill the box, never send it. */
  it("hands the composer a sentence about the selection and does not send it", () => {
    const onAsk = vi.fn();
    const host = canvas({ onAsk });
    select(host, 0, 11);
    act(() => (host.querySelector(".canvas-ask-go") as HTMLButtonElement).click());
    expect(onAsk).toHaveBeenCalledWith("About line 1 of main.rs: ");
  });

  /* ------------------------------------------------------- saving state */

  /** "Saved" that never goes away is furniture; the resting state says
   *  nothing, and the line count keeps its place. */
  it("says nothing while there is nothing to say", () => {
    const host = canvas();
    expect(host.querySelector(".canvas-state")).toBeNull();
    expect(host.querySelector(".canvas-count")?.textContent).toBe("5 lines");
  });

  it("says where the save has got to, and gives up the line count for it", () => {
    for (const [state, word] of [
      ["typing", "Unsaved"],
      ["saving", "Saving…"],
      ["failed", "Not saved"],
    ] as const) {
      const host = canvas({ state });
      expect(host.querySelector(".canvas-state")?.textContent).toBe(word);
      expect(host.querySelector(".canvas-count")).toBeNull();
    }
  });

  /** The editor shows what has been typed, not what is on disk. */
  it("shows the buffer rather than the file", () => {
    const host = canvas({ draft: "typed over it\n" });
    expect(box(host).value).toBe("typed over it\n");
  });

  /** A preview of the disk version beside an editor holding a newer one would
   *  be two answers to the same question on one screen. */
  it("previews the buffer too", () => {
    const host = canvas({
      path: "README.md",
      document: doc("README.md", PROSE),
      draft: "# Something else entirely\n",
    });
    expect(host.querySelector(".canvas-preview")?.textContent).toContain(
      "Something else entirely",
    );
  });

  /* ---------------------------------------------------------- conflicts */

  it("draws nothing about conflicts while there is not one", () => {
    expect(canvas().querySelector(".canvas-conflict")).toBeNull();
  });

  /**
   * The rule the whole slice is built around: both versions are kept and
   * neither is chosen. The editor still holds the typing, and the bar says so.
   */
  it("keeps both versions and asks, rather than choosing", () => {
    const host = canvas({
      draft: "mine\n",
      conflict: doc("src/main.rs", "theirs\n"),
    });
    const bar = host.querySelector(".canvas-conflict");
    expect(bar?.textContent).toContain("Taurus changed this file while you were typing");
    // The typing is still there — the point of not choosing.
    expect(box(host).value).toBe("mine\n");
    expect(bar?.textContent).toContain("still here, unsaved");
    // And it must not claim a loss that did not happen: the save was refused,
    // so nothing was written over anything.
    expect(bar?.textContent).not.toContain("over it");
  });

  it("offers both ways out of a conflict", () => {
    const onKeepMine = vi.fn();
    const onTakeTheirs = vi.fn();
    const host = canvas({
      draft: "mine\n",
      conflict: doc("src/main.rs", "theirs\n"),
      onKeepMine,
      onTakeTheirs,
    });
    const buttons = [...host.querySelectorAll(".canvas-conflict button")];
    act(() =>
      (buttons.find((b) => b.textContent === "Take theirs") as HTMLButtonElement).click(),
    );
    act(() =>
      (buttons.find((b) => b.textContent === "Keep mine") as HTMLButtonElement).click(),
    );
    expect(onTakeTheirs).toHaveBeenCalled();
    expect(onKeepMine).toHaveBeenCalled();
  });

  /** Above the editor rather than over it: what is being decided about has to
   *  stay readable while the decision is made. */
  it("does not cover the document it is asking about", () => {
    const host = canvas({
      draft: "mine\n",
      conflict: doc("src/main.rs", "theirs\n"),
    });
    expect(host.querySelector(".doc-input")).not.toBeNull();
  });

  it("closes on the close button", () => {
    const onClose = vi.fn();
    const host = canvas({ onClose });
    act(() =>
      (host.querySelector(".canvas-close") as HTMLButtonElement).click(),
    );
    expect(onClose).toHaveBeenCalled();
  });
});

describe("the draft a selection offers", () => {
  /** A pointer and not a quote: the lines themselves travel on the prompt, so
   *  putting them in the box would show the user their own file where their
   *  question should be, and send it twice. */
  it("names the place and leaves the question to the person", () => {
    expect(draftFor("README.md", { from: 4, to: 9, text: "..." })).toBe(
      "About lines 4–9 of README.md: ",
    );
    expect(draftFor("main.rs", { from: 4, to: 4, text: "..." })).toBe(
      "About line 4 of main.rs: ",
    );
  });

  it("does not quote the selection back", () => {
    expect(draftFor("a.md", { from: 1, to: 2, text: "secret text" })).not.toContain(
      "secret",
    );
  });
});

describe("the editor", () => {
  function editor(over: Partial<Parameters<typeof DocumentEditor>[0]> = {}) {
    return mount(
      <DocumentEditor
        text={CODE}
        path="src/main.rs"
        reveal={null}
        flash={null}
        onSelect={noop}
        {...over}
      />,
    );
  }

  /** The property the whole two-layer trick rests on: what is painted is
   *  exactly what is in the box, character for character. Anything else slides
   *  the colour off the text and keeps sliding. */
  it("paints exactly the text it holds", () => {
    const host = editor();
    const painted = [...host.querySelectorAll(".doc-ink span")]
      .map((s) => s.textContent)
      .join("");
    expect(painted).toBe(CODE);
  });

  it("numbers every line", () => {
    const numbers = [...editor().querySelectorAll(".doc-line-no")].map(
      (n) => n.textContent,
    );
    expect(numbers).toEqual(["1", "2", "3", "4", "5"]);
  });

  it("takes what is typed into it", () => {
    const onChange = vi.fn();
    const host = editor({ onChange });
    const area = box(host);
    expect(area.readOnly).toBe(false);
    const set = Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      "value",
    )!.set!;
    act(() => {
      set.call(area, "typed");
      area.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(onChange).toHaveBeenCalledWith("typed");
  });

  /** The band is behind the text rather than a class on the painted runs,
   *  because a changed *line* is not a token boundary. */
  it("tints the lines somebody else just changed", () => {
    expect(editor().querySelector(".doc-flash")).toBeNull();
    expect(
      editor({ flash: { from: 2, to: 3 } }).querySelector(".doc-flash"),
    ).not.toBeNull();
  });

  /**
   * Nothing wraps, and the gutter is why — a wrapped line is more than one row
   * and a gutter is one number per row, so wrapping walks the numbers off their
   * own lines. Asserted on the stylesheet's behalf: this is the one property
   * that keeps a line number honest, and it is the sort of thing a later
   * "prose should wrap" change would undo without noticing.
   */
  it("never wraps, so a row is always a line", () => {
    expect(editor().querySelector(".doc-editor")?.className).not.toContain("wrap");
    expect(document.querySelector(".doc-editor.wrap")).toBeNull();
  });

  /** "The retry logic is here" is a sentence about a place. */
  it("selects the lines it is sent to", () => {
    const host = editor({ reveal: { from: 2, to: 2 } });
    const area = box(host);
    expect(area.value.slice(area.selectionStart, area.selectionEnd)).toBe(
      "    // three lines in",
    );
  });

  /** A model that pointed past the end has misremembered something; the editor
   *  clamps rather than throwing, and the tool call says so in its own words. */
  it("clamps a range that runs past the end of the file", () => {
    const host = editor({ reveal: { from: 900, to: 900 } });
    const area = box(host);
    expect(area.selectionStart).toBeLessThanOrEqual(CODE.length);
    expect(area.selectionEnd).toBeLessThanOrEqual(CODE.length);
  });

  it("reports what it revealed as a selection", () => {
    const onSelect = vi.fn();
    editor({ reveal: { from: 2, to: 3 }, onSelect });
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ from: 2, to: 3 }),
    );
  });
});
