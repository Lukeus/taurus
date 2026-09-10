import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { NoteCard } from "./NoteCard";

const view = (scope: "workspace" | "global") => ({
  type: "note" as const,
  scope,
  name: "Auth redesign",
});

describe("a note card", () => {
  it("names the note and the notebook it is in", () => {
    expect(renderToStaticMarkup(<NoteCard view={view("workspace")} />)).toContain("project note");
    expect(renderToStaticMarkup(<NoteCard view={view("global")} />)).toContain("global note");
  });

  it("says where the file is on hover, because that is what says whether it is committed", () => {
    expect(renderToStaticMarkup(<NoteCard view={view("workspace")} />)).toContain(
      'data-tip=".taurus/notes/Auth redesign.md"',
    );
    expect(renderToStaticMarkup(<NoteCard view={view("global")} />)).toContain(
      'data-tip="~/.taurus/notes/Auth redesign.md"',
    );
  });

  it("offers Open only where there is a pane to open it in", () => {
    // A delegate's transcript is read on its own and has no notes pane.
    expect(renderToStaticMarkup(<NoteCard view={view("workspace")} onOpen={() => {}} />)).toContain(
      ">Open<",
    );
    expect(renderToStaticMarkup(<NoteCard view={view("workspace")} />)).not.toContain("<button");
  });

  it("carries nothing that could go stale", () => {
    // The card is the call's input and no more: a notebook, a name. The note
    // itself is read when it is opened, so a month-old card opens today's note.
    expect(Object.keys(view("workspace")).sort()).toEqual(["name", "scope", "type"]);
  });
});
