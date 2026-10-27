// @vitest-environment jsdom
import { describe, expect, it } from "vitest";

import { terminalFont } from "./TerminalDock";

describe("the terminal's font", () => {
  it("puts a chosen face ahead of the app's stack, not instead of it", () => {
    document.documentElement.style.setProperty("--mono-terminal", "'Symbols Nerd Font', monospace");
    expect(terminalFont("JetBrains Mono")).toBe(`"JetBrains Mono", 'Symbols Nerd Font', monospace`);
    expect(terminalFont("  ")).toBe("'Symbols Nerd Font', monospace");
    // A quote typed into the field can't close the family name early.
    expect(terminalFont('Fira "Code"')).toBe(`"Fira Code", 'Symbols Nerd Font', monospace`);
  });
});
