// @vitest-environment jsdom
//
// `APPLE` is read from the user agent once, at import — which is the right
// thing for a value that cannot change while the window is open, and means a
// test for the other platform has to reload the module.
import { afterEach, describe, expect, it, vi } from "vitest";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});

/** The module as it loads on a machine claiming to be `agent`. */
async function on(agent: string) {
  vi.stubGlobal("navigator", { userAgent: agent });
  vi.resetModules();
  return import("./keys");
}

const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)";
const WINDOWS = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

const press = (init: KeyboardEventInit) => new KeyboardEvent("keydown", init);

describe("which modifier a shortcut is written with", () => {
  it("is Command on a Mac and Control everywhere else", async () => {
    expect((await on(MAC)).chord("K")).toBe("⌘K");
    expect((await on(WINDOWS)).chord("K")).toBe("Ctrl+K");
  });
});

describe("recognising a chord", () => {
  it("watches the key the label promises", async () => {
    // The pair that goes wrong, and the reason both halves live in one file:
    // a row reading ⌘K beside a handler watching `ctrlKey` is a shortcut that
    // is documented and does not work, and neither file shows it alone.
    const mac = await on(MAC);
    expect(mac.isChord(press({ key: "k", metaKey: true }), "k")).toBe(true);
    expect(mac.isChord(press({ key: "k", ctrlKey: true }), "k")).toBe(false);

    const windows = await on(WINDOWS);
    expect(windows.isChord(press({ key: "k", ctrlKey: true }), "k")).toBe(true);
    expect(windows.isChord(press({ key: "k", metaKey: true }), "k")).toBe(false);
  });

  it("ignores the case the key arrives in", async () => {
    // Shift is not excluded — ⌘⇧P is a chord this has to recognise — and with
    // Shift held the browser reports `K` rather than `k`.
    const mac = await on(MAC);
    expect(mac.isChord(press({ key: "K", metaKey: true, shiftKey: true }), "k")).toBe(
      true,
    );
  });

  it("refuses the other modifier as well as the wrong one", async () => {
    // ⌃⌘K and ⌘K are different chords, and a handler that fires on both is
    // one that steals a shortcut somebody's window manager has.
    const mac = await on(MAC);
    expect(mac.isChord(press({ key: "k", metaKey: true, ctrlKey: true }), "k")).toBe(
      false,
    );
    expect(mac.isChord(press({ key: "k", metaKey: true, altKey: true }), "k")).toBe(
      false,
    );
  });

  it("refuses a bare key", async () => {
    const mac = await on(MAC);
    expect(mac.isChord(press({ key: "k" }), "k")).toBe(false);
  });

  it("refuses a different key", async () => {
    const mac = await on(MAC);
    expect(mac.isChord(press({ key: "j", metaKey: true }), "k")).toBe(false);
  });
});
