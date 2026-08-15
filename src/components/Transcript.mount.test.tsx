// @vitest-environment jsdom
//
// The streaming terminal is stateful in ways a first paint cannot show: it
// follows the output while a command runs, and has to keep showing something
// once it stops. Both need a real document and a component that stays mounted.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { Transcript } from "./Transcript";
import type { Entry } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// jsdom lays nothing out and implements neither of these. The transcript pins
// itself to the bottom on every render, so without the stub every test here
// fails on the scroll rather than on what it is testing.
Element.prototype.scrollIntoView = () => {};

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

function mount(entries: Entry[]) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => {
    root.render(<Transcript entries={entries} busy={false} empty={null} />);
  });
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return {
    host,
    rerender: (next: Entry[]) =>
      act(() => {
        root.render(<Transcript entries={next} busy={false} empty={null} />);
      }),
  };
}

const command = (patch: Partial<Extract<Entry, { kind: "tool" }>> = {}): Entry => ({
  kind: "tool",
  id: "t1",
  name: "run_command",
  preview: "Run: cargo build",
  status: "running",
  steps: [],
  ...patch,
});

describe("a running command", () => {
  it("shows what it has printed before it finishes", () => {
    const { host } = mount([
      command({ steps: ["   Compiling taurus-core\n", "   Compiling taurus-cli\n"] }),
    ]);

    const stream = host.querySelector(".tool-stream");
    expect(stream).not.toBeNull();
    expect(stream!.textContent).toContain("Compiling taurus-core");
    expect(stream!.textContent).toContain("Compiling taurus-cli");
    // Marked live, which is what tells it apart from a finished result.
    expect(stream!.className).toContain("live");
  });

  it("keeps its output on screen once it exits", () => {
    // The row must not go blank at the moment the command ends — that reads as
    // the output having been lost.
    const { host, rerender } = mount([command({ steps: ["building…\n"] })]);
    rerender([
      command({
        status: "ok",
        steps: ["building…\n"],
        output: "building…\nFinished in 3.1s",
      }),
    ]);

    const stream = host.querySelector(".tool-stream")!;
    // The authoritative result replaces the streamed copy in the same place.
    expect(stream.textContent).toContain("Finished in 3.1s");
    expect(stream.className).not.toContain("live");
  });

  it("does not draw an empty terminal before anything is printed", () => {
    const { host } = mount([command()]);
    expect(host.querySelector(".tool-stream")).toBeNull();
  });
});

describe("every other tool", () => {
  it("keeps its result behind the row rather than streaming it", () => {
    // A read is instant. Streaming every one of them would bury the sentences
    // either side of the run.
    const { host } = mount([
      {
        kind: "tool",
        id: "t2",
        name: "read_file",
        preview: "Read src/main.rs",
        status: "ok",
        steps: [],
        output: "fn main() {}",
      },
    ]);

    expect(host.querySelector(".tool-stream")).toBeNull();
    expect(host.textContent).toContain("Read src/main.rs");
    expect(host.textContent).not.toContain("fn main()");
  });
});
