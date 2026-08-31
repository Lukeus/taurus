// @vitest-environment jsdom
//
// What the box does that a first paint cannot show: what it holds on to when
// the conversation under it changes, what Enter does while a turn is running,
// and what the line beneath it says about which of those is about to happen.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { Composer, type Parked } from "./App";
import type { Attachment, OnScreen } from "./lib/api";
import type { Outgoing } from "./state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

type Options = {
  sessionKey?: string;
  parked?: Parked | null;
  busy?: boolean;
  queued?: Outgoing | null;
  focus?: number;
};

function mount(options: Options = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const sent: { text: string; images: Attachment[]; onScreen: OnScreen | null }[] =
    [];
  const parked: { key: string; held: Parked }[] = [];
  const unqueued: true[] = [];
  const sentQueued: true[] = [];

  const render = (opts: Options) =>
    root.render(
      <Composer
        key={opts.sessionKey ?? "s1"}
        sessionKey={opts.sessionKey ?? "s1"}
        parked={opts.parked ?? null}
        onPark={(key, held) => parked.push({ key, held })}
        queued={opts.queued ?? null}
        onSendQueued={() => sentQueued.push(true)}
        onUnqueue={() => unqueued.push(true)}
        focus={opts.focus ?? 0}
        busy={opts.busy ?? false}
        stopping={false}
        ready
        vision={false}
        workspace="/src/taurus"
        library="0:0"
        onScreen={null}
        draft={null}
        onPickWorkspace={() => {}}
        onSend={(text, images, onScreen) => sent.push({ text, images, onScreen })}
        onStop={() => {}}
        onUsage={() => {}}
      />,
    );

  act(() => render(options));
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });

  const box = () => host.querySelector("textarea") as HTMLTextAreaElement;
  return {
    host,
    sent,
    parked,
    unqueued,
    sentQueued,
    box,
    rerender: (opts: Options) => act(() => render({ ...options, ...opts })),
    type: (text: string) =>
      act(() => {
        const el = box();
        // React listens on the property setter, which assigning to `.value`
        // goes around. This is the documented way to drive a controlled input.
        const set = Object.getOwnPropertyDescriptor(
          HTMLTextAreaElement.prototype,
          "value",
        )!.set!;
        set.call(el, text);
        el.dispatchEvent(new Event("input", { bubbles: true }));
      }),
    enter: () =>
      act(() => {
        box().dispatchEvent(
          new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
        );
      }),
    click: (element: Element | null) =>
      act(() => {
        (element as HTMLElement).click();
      }),
    hint: () => host.querySelector(".composer-hint")?.textContent ?? "",
  };
}

describe("a half-written question and a change of conversation", () => {
  it("hands what was typed back on the way out", () => {
    const { type, parked } = mount({ sessionKey: "s1" });
    type("where is the build time");
    cleanup.pop()?.();

    expect(parked).toEqual([
      { key: "s1", held: { text: "where is the build time", images: [] } },
    ]);
  });

  it("parks it under the conversation it was typed into", () => {
    /*
     * The bug this whole arrangement exists for. The key has to be captured at
     * mount: read from a ref during teardown it would already name the
     * conversation being switched *to*, and every draft would come back under
     * the wrong one.
     */
    const { type, parked, rerender } = mount({ sessionKey: "s1" });
    type("meant for the first one");
    rerender({ sessionKey: "s2" });

    expect(parked.map((p) => p.key)).toEqual(["s1"]);
    expect(parked[0].held.text).toBe("meant for the first one");
  });

  it("does not carry it into the next conversation", () => {
    // What used to happen: the composer outlived the transcript under it, so a
    // sentence went to whichever conversation happened to be open when Enter
    // was finally pressed.
    const { type, rerender, box } = mount({ sessionKey: "s1" });
    type("meant for the first one");
    rerender({ sessionKey: "s2" });

    expect(box().value).toBe("");
  });

  it("gives it back when that conversation comes back", () => {
    const { box } = mount({
      sessionKey: "s1",
      parked: { text: "where is the build time", images: [] },
    });
    expect(box().value).toBe("where is the build time");
  });
});

describe("Enter while a turn is running", () => {
  it("sends rather than doing nothing", () => {
    // It used to return early and silently. The store is what decides to hold
    // it; the box's job is to stop swallowing the keystroke.
    const { type, enter, sent } = mount({ busy: true });
    type("and then profile it");
    enter();

    expect(sent.map((s) => s.text)).toEqual(["and then profile it"]);
  });

  it("says so before it is pressed", () => {
    const { type, hint } = mount({ busy: true });
    expect(hint()).toContain("↵ send");
    type("and then profile it");
    expect(hint()).toContain("sends when this turn ends");
  });

  it("goes back to saying send once the turn is over", () => {
    const { type, rerender, hint } = mount({ busy: true });
    type("and then profile it");
    rerender({ busy: false });
    expect(hint()).not.toContain("when this turn ends");
  });
});

const HELD: Outgoing = {
  text: "and then profile it",
  images: [],
  onScreen: null,
};

describe("a message waiting its turn", () => {
  it("is drawn, because a silent promise is what this replaced", () => {
    const { host } = mount({ busy: true, queued: HELD });
    expect(host.querySelector(".composer-queued")?.textContent).toContain(
      "and then profile it",
    );
  });

  it("can be thrown away", () => {
    const { host, click, unqueued } = mount({ busy: true, queued: HELD });
    click(host.querySelector('[aria-label="Discard this message"]'));
    expect(unqueued).toHaveLength(1);
  });

  it("offers no hand-send while the turn it is behind is still running", () => {
    // It goes on its own the moment that turn finishes cleanly. A button here
    // would be offering to do the thing that is already going to happen.
    const { host } = mount({ busy: true, queued: HELD });
    expect(host.textContent).not.toContain("Send it");
  });

  it("offers one once that turn has ended without taking it", () => {
    // Stopped, or died. Either way it did not go, and the alternative to an
    // automatic resend has to be one click rather than retyping the sentence.
    const { host, click, sentQueued } = mount({ busy: false, queued: HELD });
    expect(host.querySelector(".composer-queued")?.className).toContain("held");
    click([...host.querySelectorAll("button")].find((b) => b.textContent === "Send it")!);
    expect(sentQueued).toHaveLength(1);
  });
});

describe("being asked for from elsewhere in the window", () => {
  it("takes the cursor when the count moves", () => {
    const { rerender, box } = mount({ focus: 0 });
    box().blur();
    rerender({ focus: 1 });
    expect(document.activeElement).toBe(box());
  });

  it("does not grab it on the first render", () => {
    // The counter starts somewhere, and a composer that stole focus on mount
    // would fight the search box and every drawer that opens over it.
    const { box } = mount({ focus: 7 });
    expect(document.activeElement).not.toBe(box());
  });
});
