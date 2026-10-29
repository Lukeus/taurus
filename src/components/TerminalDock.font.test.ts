// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

import { families } from "../lib/fixedPitch";
import { terminalFont } from "./TerminalDock";

describe("the terminal's font", () => {
  it("puts a chosen face ahead of the app's stack, not instead of it", () => {
    document.documentElement.style.setProperty("--mono-terminal", "'Symbols Nerd Font', monospace");
    expect(terminalFont("JetBrains Mono")).toBe(`"JetBrains Mono", 'Symbols Nerd Font', monospace`);
    expect(terminalFont("  ")).toBe("'Symbols Nerd Font', monospace");
    // A quote typed into the field can't close the family name early.
    expect(terminalFont('Fira "Code"')).toBe(`"Fira Code", 'Symbols Nerd Font', monospace`);
  });

  it("leaves out a face that isn't fixed-width", async () => {
    // A canvas whose letters are as wide as the family says: "Segoe UI" is a
    // proportional face, everything else measures the same for "i" and "W".
    class Canvas {
      getContext() {
        let font = "";
        return {
          set font(value: string) {
            font = value;
          },
          get font() {
            return font;
          },
          measureText: (text: string) => ({
            width: text.length * (font.includes("Segoe UI") && text[0] === "i" ? 4 : 10),
          }),
        };
      }
    }
    vi.stubGlobal("OffscreenCanvas", Canvas);
    vi.resetModules();
    const { terminalFont: measured } = await import("./TerminalDock");
    document.documentElement.style.setProperty(
      "--mono-terminal",
      `"Segoe UI", "Symbols Nerd Font Mono", Consolas, monospace`,
    );
    expect(measured("")).toBe(`"Symbols Nerd Font Mono", Consolas, monospace`);
    expect(measured("Segoe UI")).toBe(`"Symbols Nerd Font Mono", Consolas, monospace`);
    expect(measured("Cascadia Code")).toBe(
      `"Cascadia Code", "Symbols Nerd Font Mono", Consolas, monospace`,
    );
  });
});

describe("a font-family list", () => {
  it("splits on the commas outside quotes", () => {
    expect(families(`"Fira, Code", 'A B' ,  monospace`)).toEqual([`"Fira, Code"`, `'A B'`, "monospace"]);
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});
