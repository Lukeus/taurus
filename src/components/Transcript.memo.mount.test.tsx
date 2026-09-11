// @vitest-environment jsdom
//
// Whether a render of the transcript that changes nothing draws anything.
//
// The turns are memoized, and a memo compares what it is handed — so one
// callback that reaches a turn without being held steady makes every turn look
// new on every render of whatever owns the transcript. `App` re-renders on
// things that have nothing to do with the conversation, and each of those used
// to redraw every turn in it. Nothing on screen changes when that happens, so
// only counting the draws can see it.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Entry } from "../state/store";

/**
 * How many times a question's pictures were drawn.
 *
 * The probe is the attachment strip because of where it sits: the question
 * heading a turn is not memoized on its own, so it is drawn exactly as often as
 * its turn is. The answers below it are memoized per entry, and counting those
 * would count nothing whether the turn redrew or not.
 */
let drawn = 0;
vi.mock("./Attachments", () => ({
  Attachments: () => {
    drawn += 1;
    return null;
  },
}));

const { Transcript } = await import("./Transcript");

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;
Element.prototype.scrollIntoView = () => {};

let cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  drawn = 0;
});

const picture = { media_type: "image/png", data: "" } as never;

const conversation: Entry[] = [0, 1, 2].flatMap((t): Entry[] => [
  { kind: "user", id: `u${t}`, text: `question ${t}`, images: [picture] },
  { kind: "assistant", id: `a${t}`, open: false, thinking: "", text: `answer ${t}` },
]);

/** The transcript as `App` draws it: every callback a fresh function, every
 *  render. That is the natural way to write a caller, and the case to hold. */
function mount(entries: Entry[]) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const render = () =>
    root.render(
      <Transcript
        entries={entries}
        busy={false}
        empty={null}
        onAnswer={() => {}}
        onOpenDelegate={() => {}}
        onOpenDataset={() => {}}
        onOpenDocument={() => {}}
        onOpenNote={() => {}}
        onRunQuery={() => {}}
        onEditPrompt={() => {}}
      />,
    );
  act(render);
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return { rerender: () => act(render) };
}

describe("a render that changes nothing", () => {
  it("draws no turn again, whatever callbacks it was handed", () => {
    const view = mount(conversation);
    expect(drawn).toBe(3);

    view.rerender();
    view.rerender();

    expect(drawn).toBe(3);
  });
});
