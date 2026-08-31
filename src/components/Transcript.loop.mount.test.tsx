// @vitest-environment jsdom
//
// The four places the transcript now answers back rather than only reporting:
// a run that folds once the conversation moves past it, a failure that offers a
// way forward, a question that can be asked again, and a way back to the foot
// of a stream somebody has scrolled up out of. All four are stateful — they
// depend on what is newest, on what the reader has already clicked, or on where
// the scroll is — so none of them can be seen in a first paint.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { Transcript } from "./Transcript";
import type { Entry } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

Element.prototype.scrollIntoView = () => {};

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

type Options = {
  busy?: boolean;
  onRetry?: () => void;
  onEditPrompt?: (text: string) => void;
};

function mount(entries: Entry[], options: Options = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const render = (next: Entry[], opts: Options) =>
    root.render(
      <Transcript
        entries={next}
        busy={opts.busy ?? false}
        empty={null}
        onAnswer={() => {}}
        onRetry={opts.onRetry}
        onEditPrompt={opts.onEditPrompt}
      />,
    );

  act(() => render(entries, options));
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return {
    host,
    click: (element: Element | null) =>
      act(() => {
        (element as HTMLElement).click();
      }),
    rerender: (next: Entry[], opts: Options = options) =>
      act(() => render(next, opts)),
    runs: () => [...host.querySelectorAll(".run")],
  };
}

const ask = (id: string, text: string): Entry => ({ kind: "user", id, text });

const ran = (id: string): Entry => ({
  kind: "tool",
  id,
  name: "read_file",
  preview: "Read src/main.rs",
  status: "ok",
  steps: [],
  output: "fn main() {}",
});

const said = (id: string, text: string): Entry => ({
  kind: "assistant",
  id,
  text,
  thinking: "",
  open: false,
});

describe("a run of tool calls", () => {
  it("shows its steps while its turn is the newest thing said", () => {
    const { runs } = mount([ask("u1", "read main"), ran("t1")]);
    expect(runs()[0].className).toContain("open");
  });

  it("folds to its heading once a newer question has been asked", () => {
    // What lets a fifty-turn conversation be read as fifty steps rather than as
    // four hundred tool calls.
    const first = [ask("u1", "read main"), ran("t1")];
    const { runs, rerender } = mount(first);
    expect(runs()[0].className).toContain("open");

    rerender([...first, ask("u2", "now the tests"), ran("t2")]);
    expect(runs()[0].className).not.toContain("open");
    expect(runs()[1].className).toContain("open");
  });

  it("stays open where it failed, wherever it ends up", () => {
    // The heading says a step failed and not which one. Folding away the
    // answer to the question the heading just raised is the one case where
    // tidiness costs more than it saves.
    const broke: Entry = { ...(ran("t1") as Extract<Entry, { kind: "tool" }>), status: "error" };
    const first = [ask("u1", "read main"), broke];
    const { runs, rerender } = mount(first);

    rerender([...first, ask("u2", "never mind"), ran("t2")]);
    expect(runs()[0].className).toContain("open");
  });

  it("never overrules a reader who has clicked", () => {
    // Both directions. A panel that reopened because a turn ended, or shut
    // while it was being read, would be the app arguing with a click.
    const first = [ask("u1", "read main"), ran("t1")];
    const { host, runs, rerender, click } = mount(first);

    click(host.querySelector(".run-head"));
    expect(runs()[0].className).not.toContain("open");

    rerender([...first, ask("u2", "now the tests"), ran("t2")]);
    // Already shut, and it stays shut rather than being shut a second time.
    expect(runs()[0].className).not.toContain("open");

    click(runs()[0].querySelector(".run-head"));
    rerender([...first, ask("u2", "now the tests"), ran("t2"), said("a1", "Done.")]);
    // Opened by hand in a turn that is no longer newest, and left that way.
    expect(runs()[0].className).toContain("open");
  });
});

const died = (id: string, text: string): Entry => ({
  kind: "notice",
  id,
  text,
  tone: "error",
  failed: true,
});

describe("a turn that died", () => {
  it("offers to send the same message again", () => {
    const tried: true[] = [];
    const { host, click } = mount(
      [ask("u1", "time the build"), died("n1", "ollama is not answering")],
      { onRetry: () => tried.push(true) },
    );

    const retry = host.querySelector(".notice-retry");
    expect(retry).not.toBeNull();
    click(retry);
    expect(tried).toHaveLength(1);
  });

  it("offers nothing on a failure the conversation has moved past", () => {
    // What a retry resends is the *last* message the conversation sent, so a
    // button on an older failure would quietly ask a different question than
    // the one it is sitting under.
    const { host } = mount(
      [
        ask("u1", "time the build"),
        died("n1", "ollama is not answering"),
        ask("u2", "try again"),
        said("a1", "110.3s."),
      ],
      { onRetry: () => {} },
    );
    expect(host.querySelector(".notice-retry")).toBeNull();
  });

  it("offers nothing where the turn carried on past the failure", () => {
    // A tool that returned an error inside a turn the model then handled is
    // not a turn waiting to be tried again.
    const { host } = mount(
      [
        ask("u1", "time the build"),
        died("n1", "the first provider refused"),
        said("a1", "Went to the other one instead."),
      ],
      { onRetry: () => {} },
    );
    expect(host.querySelector(".notice-retry")).toBeNull();
  });

  it("says nothing at all where there is nowhere to send it back to", () => {
    // A delegate's transcript, which nobody is typing at.
    const { host } = mount([
      ask("u1", "time the build"),
      died("n1", "ollama is not answering"),
    ]);
    expect(host.querySelector(".notice-retry")).toBeNull();
  });
});

describe("asking something again", () => {
  it("hands the question back rather than editing the record", () => {
    // The transcript is what was actually asked and answered. Rewriting a
    // question the model already read would make it a record of something that
    // did not happen.
    const back: string[] = [];
    const { host, click } = mount(
      [ask("u1", "where is the build time going?"), said("a1", "110.3s.")],
      { onEditPrompt: (text) => back.push(text) },
    );

    click(host.querySelector(".prompt-edit"));
    expect(back).toEqual(["where is the build time going?"]);
    // Still there, unchanged.
    expect(host.querySelector(".prompt-text")?.textContent).toBe(
      "where is the build time going?",
    );
  });

  it("is offered on every question, not only the newest", () => {
    const { host } = mount(
      [ask("u1", "first"), said("a1", "ok"), ask("u2", "second")],
      { onEditPrompt: () => {} },
    );
    expect(host.querySelectorAll(".prompt-edit")).toHaveLength(2);
  });
});

describe("the way back to the foot of a stream", () => {
  /** jsdom lays nothing out, so the scroll geometry has to be stated. */
  const scrollTo = (el: Element, fromBottom: number) => {
    Object.defineProperty(el, "scrollHeight", { value: 1000, configurable: true });
    Object.defineProperty(el, "clientHeight", { value: 400, configurable: true });
    Object.defineProperty(el, "scrollTop", {
      value: 1000 - 400 - fromBottom,
      configurable: true,
    });
    act(() => {
      el.dispatchEvent(new Event("scroll"));
    });
  };

  it("is absent while the view is at the foot", () => {
    const { host } = mount([ask("u1", "go"), said("a1", "ok")], { busy: true });
    expect(host.querySelector(".to-foot")).toBeNull();
  });

  it("appears once the reader has scrolled up out of it", () => {
    const { host } = mount([ask("u1", "go"), said("a1", "ok")], { busy: true });
    scrollTo(host.querySelector(".transcript")!, 300);
    expect(host.querySelector(".to-foot")).not.toBeNull();
  });

  it("goes again when the view comes back", () => {
    const { host } = mount([ask("u1", "go"), said("a1", "ok")], { busy: true });
    const transcript = host.querySelector(".transcript")!;
    scrollTo(transcript, 300);
    expect(host.querySelector(".to-foot")).not.toBeNull();
    scrollTo(transcript, 10);
    expect(host.querySelector(".to-foot")).toBeNull();
  });

  it("says which of the two things it is offering", () => {
    // Catching up with something still arriving is a different errand from
    // walking back down a conversation that has stopped.
    const { host, rerender } = mount([ask("u1", "go"), said("a1", "ok")], {
      busy: true,
    });
    scrollTo(host.querySelector(".transcript")!, 300);
    expect(host.querySelector(".to-foot")?.textContent).toContain("live edge");

    rerender([ask("u1", "go"), said("a1", "ok")], { busy: false });
    expect(host.querySelector(".to-foot")?.textContent).toContain("the end");
  });

  it("is not offered on a transcript nothing is writing to", () => {
    // A delegate's record is read from the top, and a control saying "jump to
    // the end" on a page that is not being written is furniture.
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    act(() =>
      root.render(
        <Transcript
          entries={[ask("u1", "go"), said("a1", "ok")]}
          busy={false}
          follow={false}
          empty={null}
          onAnswer={() => {}}
        />,
      ),
    );
    cleanup.push(() => {
      act(() => root.unmount());
      host.remove();
    });

    scrollTo(host.querySelector(".transcript")!, 300);
    expect(host.querySelector(".to-foot")).toBeNull();
  });
});
