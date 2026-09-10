import { describe as suite, expect, it } from "vitest";

import { describe, fromView, mermaid, plan } from "./sequence";
import type { TranscriptView } from "./api";

type SequenceView = Extract<TranscriptView, { type: "sequence" }>;

const view = (patch: Partial<SequenceView> = {}): SequenceView => ({
  type: "sequence",
  title: "Placing an order",
  caption: null,
  participants: ["Client", "API", "Store"],
  messages: [
    { from: "Client", to: "API", text: "POST /orders", kind: "call" },
    { from: "API", to: "Store", text: "insert row", kind: "call" },
    { from: "Store", to: "API", text: "ok", kind: "return" },
  ],
  ...patch,
});

/** These cases are all about a `show_sequence` payload, where a lane's key is
 *  the name drawn above it. See `fromView`. */
const layout = (v: SequenceView) => plan(fromView(v));
const described = (v: SequenceView) => describe(fromView(v));
const asMermaid = (v: SequenceView) => mermaid(fromView(v));

suite("laying out a sequence diagram", () => {
  it("spaces the lanes evenly in the order they were declared", () => {
    const { lanes } = layout(view());
    expect(lanes.map((l) => l.label)).toEqual(["Client", "API", "Store"]);
    const gaps = lanes.slice(1).map((lane, i) => lane.x - lanes[i].x);
    expect(new Set(gaps).size).toBe(1);
  });

  it("widens the lanes to fit the longest name rather than clipping it", () => {
    const narrow = layout(view());
    const wide = layout(
      view({ participants: ["Client", "API", "OrderProjectionStore"] }),
    );
    expect(wide.lanes[1].x - wide.lanes[0].x).toBeGreaterThan(
      narrow.lanes[1].x - narrow.lanes[0].x,
    );
  });

  it("puts each message on its own row, in order", () => {
    const { rows } = layout(view());
    expect(rows.map((r) => r.text)).toEqual([
      "POST /orders",
      "insert row",
      "ok",
    ]);
    const ys = rows.map((r) => r.y);
    expect([...ys].sort((a, b) => a - b)).toEqual(ys);
  });

  it("gives a self-call the extra height its loop needs", () => {
    const straight = layout(view());
    const looping = layout(
      view({
        messages: [
          { from: "API", to: "API", text: "validate the body", kind: "call" },
          ...view().messages,
        ],
      }),
    );
    expect(looping.rows[0].self).toBe(true);
    expect(looping.height).toBeGreaterThan(straight.height + 38);
  });

  it("makes room for a label wider than the arrow under it", () => {
    // A long label centred over a short arrow in the leftmost lane runs off
    // the left of the drawing; a viewBox starting at zero would halve it.
    const { left } = layout(
      view({
        participants: ["A", "B"],
        messages: [
          {
            from: "A",
            to: "B",
            text: "a label considerably wider than the arrow it sits above",
            kind: "call",
          },
        ],
      }),
    );
    expect(left).toBeLessThan(0);
  });

  it("drops an arrow naming a lane that does not exist", () => {
    // The tool refuses these outright, so this is only reachable from a
    // transcript written by some other build. One arrow short beats a card
    // that throws and takes the whole transcript with it.
    const { rows } = layout(
      view({
        messages: [
          { from: "Client", to: "Ghost", text: "nowhere", kind: "call" },
          { from: "Client", to: "API", text: "POST /orders", kind: "call" },
        ],
      }),
    );
    expect(rows.map((r) => r.text)).toEqual(["POST /orders"]);
  });
});

suite("copying a sequence as Mermaid", () => {
  it("round-trips the arrows and their direction", () => {
    const text = asMermaid(view());
    expect(text.startsWith("sequenceDiagram")).toBe(true);
    expect(text).toContain("p0->>p1: POST /orders");
    // A return is Mermaid's dashed arrow, not a second solid one.
    expect(text).toContain("p2-->>p1: ok");
  });

  it("aliases participants so a name with a space cannot break the parse", () => {
    // Mermaid splits a message line on its arrow, so `Order Store->>API` is
    // either a parse error or a lane nobody declared.
    const text = asMermaid(view({ participants: ["Client", "API", "Order Store"] }));
    expect(text).toContain("participant p2 as Order Store");
    expect(text).not.toContain("Order Store->>");
  });
});

suite("describing a sequence for a screen reader", () => {
  it("says the order of events, which is what the picture shows", () => {
    const text = described(view());
    expect(text).toContain("Placing an order");
    expect(text).toContain("Client to API: POST /orders");
    expect(text).toContain("Store returns to API: ok");
  });

  it("reads a self-call as the participant doing it", () => {
    const text = described(
      view({
        messages: [{ from: "API", to: "API", text: "validates the body", kind: "call" }],
      }),
    );
    expect(text).toContain("API validates the body");
  });
});

suite("a participant's key and its label", () => {
  const twins = {
    title: "Two of them",
    participants: [
      { key: "p0", label: "Worker" },
      { key: "p1", label: "Worker" },
    ],
    messages: [
      { from: "p0", to: "p1", text: "hands off", kind: "call" as const },
    ],
  };

  it("keeps two lanes that read the same apart", () => {
    // `show_sequence` names a lane by the string drawn above it. Mermaid has
    // ids, and two `participant … as Worker` lines are two lanes.
    const { lanes, rows } = plan(twins);
    expect(lanes.map((l) => l.label)).toEqual(["Worker", "Worker"]);
    expect(rows).toHaveLength(1);
    expect(rows[0].self).toBe(false);
    expect(rows[0].from).not.toBe(rows[0].to);
  });

  it("names both by their label when describing them", () => {
    expect(describe(twins)).toContain("Worker to Worker: hands off");
  });

  it("aliases by key when emitting Mermaid, so both survive the round trip", () => {
    const text = mermaid(twins);
    expect(text).toContain("participant p0 as Worker");
    expect(text).toContain("participant p1 as Worker");
    expect(text).toContain("p0->>p1: hands off");
  });
});
