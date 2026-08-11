import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import { ChangesDrawer, Outcome } from "./ChangesDrawer";

describe("restore outcomes", () => {
  it("names the file it put back", () => {
    const html = renderToStaticMarkup(
      <Outcome outcome={{ action: "reverted", path: "src/main.rs" }} />,
    );
    expect(html).toContain("src/main.rs");
    expect(html).toContain("reverted");
  });

  it("distinguishes a file that was deleted rather than reverted", () => {
    // A turn that created a file is undone by removing it, and that reads very
    // differently to the user than "reverted".
    const html = renderToStaticMarkup(
      <Outcome outcome={{ action: "deleted", path: "src/new.rs" }} />,
    );
    expect(html).toContain("deleted");
    expect(html).not.toContain("reverted");
  });

  it("gives the reason a file could not be restored", () => {
    // The one outcome the user must not skim past: the rewind reported
    // success overall but this file is still as the model left it.
    const html = renderToStaticMarkup(
      <Outcome
        outcome={{
          action: "skipped",
          path: "assets/logo.png",
          reason: "was not text when it was recorded",
        }}
      />,
    );
    expect(html).toContain("assets/logo.png");
    expect(html).toContain("was not text when it was recorded");
    expect(html).toContain("warn");
  });
});

describe("rendering", () => {
  it("survives a first paint before the checkpoint list has loaded", () => {
    const html = renderToStaticMarkup(
      <ChangesDrawer sessionId="s1" busy={false} onClose={() => {}} />,
    );
    expect(html).toContain("Changes");
  });

  it("shows no empty-state message until the list has actually arrived", () => {
    // `null` means "not loaded"; only an empty array means "nothing here".
    // Conflating them flashes "no changes" on every open.
    const html = renderToStaticMarkup(
      <ChangesDrawer sessionId="s1" busy={false} onClose={() => {}} />,
    );
    expect(html).not.toContain("has not changed any files");
  });
});
