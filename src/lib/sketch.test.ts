import { describe, expect, it } from "vitest";

import { parse } from "./sketch";
import { sketchName } from "../components/SketchEmbed";

describe("reading a sketch file", () => {
  it("hands Excalidraw the scene, told to bring it into view", () => {
    const got = parse('{"type":"excalidraw","elements":[{"id":"a"}],"appState":{"gridSize":20}}');
    expect(got.ok).toBe(true);
    if (!got.ok) return;
    expect(got.data.elements).toHaveLength(1);
    expect(got.data.appState).toEqual({ gridSize: 20 });
    // A sketch opened scrolled to wherever its author left the viewport can
    // open onto empty canvas with the drawing off to one side.
    expect(got.data.scrollToContent).toBe(true);
  });

  it("fills in what a hand-written scene leaves out", () => {
    const got = parse('{"elements":[]}');
    expect(got.ok && got.data.files).toEqual({});
    expect(got.ok && got.data.appState).toEqual({});
  });

  it("refuses what is not JSON, and says so", () => {
    const got = parse("{ not json");
    expect(got.ok).toBe(false);
    expect(!got.ok && got.why).toContain("not valid JSON");
  });

  it("refuses JSON that is not a drawing, rather than opening an empty canvas over it", () => {
    // The refusal is the whole point of the function. An empty canvas opened
    // over a file this could not read would save itself over that file.
    for (const text of ["[]", "null", '{"type":"excalidraw"}', '{"elements":"no"}']) {
      const got = parse(text);
      expect(got.ok, text).toBe(false);
    }
  });
});

describe("which images in a note are sketches", () => {
  it("reads the name out of either spelling", () => {
    // `<Auth flow.excalidraw>` reaches the image percent-encoded.
    expect(sketchName("Auth%20flow.excalidraw")).toBe("Auth flow");
    expect(sketchName("flow.excalidraw")).toBe("flow");
    expect(sketchName("./flow.excalidraw")).toBe("flow");
  });

  it("leaves every other image alone", () => {
    for (const src of [undefined, "", "shot.png", ".excalidraw", "flow.excalidraw.png"]) {
      expect(sketchName(src), String(src)).toBeNull();
    }
  });

  it("does not reach outside the notebook the note is in", () => {
    for (const src of ["../other/far.excalidraw", "sub/flow.excalidraw", "..%2Ffar.excalidraw"]) {
      expect(sketchName(src), src).toBeNull();
    }
  });

  it("gives up on an address it cannot decode rather than throwing in the middle of a render", () => {
    expect(sketchName("%E0%A4%A.excalidraw")).toBeNull();
  });
});
