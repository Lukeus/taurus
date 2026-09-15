import { describe, expect, it } from "vitest";

import { locate, place } from "./scrollMap";

describe("carrying a position between Write and Read", () => {
  // Write: two headings, at 100 and 400, in a note 1000 tall that scrolls 700.
  const WRITE = [0, 100, 400, 1000];
  // Read: the same two headings, with a tall diagram between them.
  const READ = [0, 80, 900, 1300];

  it("keeps the same heading at the top", () => {
    expect(place(locate(100, WRITE, 700), READ, 1000)).toBe(80);
    expect(place(locate(400, WRITE, 700), READ, 1000)).toBe(900);
  });

  it("keeps how far between two headings", () => {
    // Halfway from the first to the second in Write is halfway in Read — past
    // most of the diagram, where the same fraction of the whole note would be
    // somewhere above it.
    expect(place(locate(250, WRITE, 700), READ, 1000)).toBe(490);
  });

  it("keeps the end the end, whatever the heights", () => {
    expect(place(locate(700, WRITE, 700), READ, 1000)).toBe(1000);
  });

  it("falls back to how far down when the two sides count different headings", () => {
    // Halfway down Write. An underlined heading Read counts and Write does not
    // would line everything after it up with the wrong one.
    const spot = locate(350, WRITE, 700);
    expect(place(spot, [0, 80, 700, 900, 1300], 1000)).toBe(500);
  });

  it("falls back when the landmarks are not in order", () => {
    expect(place(locate(350, WRITE, 700), [0, 900, 80, 1300], 1000)).toBe(500);
  });

  it("never puts a view past either end", () => {
    expect(place(locate(0, WRITE, 700), READ, 1000)).toBe(0);
    expect(place(locate(250, WRITE, 700), READ, 0)).toBe(0);
    expect(place(locate(250, WRITE, 700), [0, 80, 5000, 6000], 300)).toBe(300);
  });
});
