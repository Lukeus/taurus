// @vitest-environment jsdom
//
// What a token costs to draw, and whether it costs more in a long conversation
// than a short one.
//
// The second half is the point. A transcript grows all day, and a renderer that
// redraws the whole thing for every token gets slower the longer you work in
// it — which is the shape of slowness people describe as "it was fine this
// morning". So these cases differ only in how much conversation sits *above*
// the turn being streamed into, and a healthy result is four numbers that do
// not climb.
//
//   pnpm bench
//
// jsdom lays nothing out and paints nothing, so the absolute numbers are a
// fraction of what a webview pays. The ratio between them is the part that
// carries over.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { bench, describe } from "vitest";

import { Transcript } from "./Transcript";
import type { Entry } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// The transcript pins itself to the bottom on every render, and jsdom
// implements neither of these.
Element.prototype.scrollIntoView = () => {};

/** A conversation of `turns` exchanges, each a question, an answer, and a run
 *  of four tool calls — the ordinary shape, not a worst case. */
function history(turns: number): Entry[] {
  const out: Entry[] = [];
  for (let t = 0; t < turns; t++) {
    out.push({ kind: "user", id: `u${t}`, text: `question ${t}`, images: [] });
    out.push({
      kind: "assistant",
      id: `a${t}`,
      open: false,
      thinking: "",
      text:
        `## Answer ${t}\n\nSome **prose** with \`code\` and a list:\n\n` +
        "- one\n- two\n- three\n\n```rust\nfn main() {}\n```\n",
    });
    for (let k = 0; k < 4; k++) {
      out.push({
        kind: "tool",
        id: `t${t}-${k}`,
        name: "read_file",
        preview: `{"path":"src/f${k}.rs"}`,
        status: "ok",
        steps: [],
        output: "ok",
        startedAt: 0,
        endedAt: 5,
      });
    }
  }
  return out;
}

/** A mounted transcript, and a way to push one more token into its last turn. */
function streaming(turns: number) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root: Root = createRoot(host);
  const base = history(turns);
  let text = "";

  const draw = (entries: Entry[]) =>
    act(() => {
      root.render(
        <Transcript
          entries={entries}
          busy={true}
          empty={null}
          onAnswer={() => {}}
          onOpenDelegate={() => {}}
        />,
      );
    });

  draw(base);

  return {
    token() {
      text += "token ";
      draw([
        ...base,
        { kind: "assistant", id: "live", open: true, thinking: "", text },
      ]);
    },
    stop() {
      act(() => root.unmount());
      host.remove();
    },
  };
}

describe("one token, drawn", () => {
  for (const turns of [1, 5, 20, 50]) {
    let live: ReturnType<typeof streaming>;
    bench(
      `under ${turns} ${turns === 1 ? "turn" : "turns"} of history`,
      () => live.token(),
      {
        setup: () => {
          live = streaming(turns);
        },
        teardown: () => live.stop(),
      },
    );
  }
});
