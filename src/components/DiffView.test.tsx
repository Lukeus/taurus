import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { FileDiff, PermissionRequest } from "../lib/api";
import { DiffView } from "./DiffView";
import { PermissionDialog } from "./PermissionDialog";

const diff = (patch: Partial<FileDiff> = {}): FileDiff => ({
  path: "src/widget.rs",
  created: false,
  added: 1,
  removed: 1,
  elided: 0,
  hunks: [
    {
      lines: [
        { kind: "context", text: "fn widget() {", old_line: 1, new_line: 1 },
        { kind: "removed", text: "    let x = 1;", old_line: 2, new_line: null },
        { kind: "added", text: "    let x = 2;", old_line: null, new_line: 2 },
      ],
    },
  ],
  ...patch,
});

const request = (patch: Partial<PermissionRequest> = {}): PermissionRequest => ({
  id: "1",
  tool: "write_file",
  effect: "write",
  preview: "Write src/widget.rs (2140 bytes)",
  diff: diff(),
  always_scope: "Allows write_file in this workspace",
  always_global_scope: "Allows write_file everywhere",
  input: {},
  ...patch,
});

describe("diff view", () => {
  it("shows both sides of a replaced line", () => {
    // The whole feature: the byte count says a file is about to be replaced
    // and nothing about what with.
    const html = renderToStaticMarkup(<DiffView diff={diff()} />);
    expect(html).toContain("let x = 1;");
    expect(html).toContain("let x = 2;");
  });

  it("marks each side with a character, not only a colour", () => {
    // Colour is the fast read and the one that fails in a screenshot, on a
    // projector, and for a reader who cannot tell red from green.
    const html = renderToStaticMarkup(<DiffView diff={diff()} />);
    expect(html).toContain(">+<");
    expect(html).toContain(">-<");
  });

  it("uses the file's own line numbers", () => {
    // So a number read off the dialog means what one read off `read_file` means.
    const html = renderToStaticMarkup(<DiffView diff={diff()} />);
    expect(html).toContain('class="diff-num" aria-hidden="true">2<');
  });

  it("says create rather than replace for a new file", () => {
    const html = renderToStaticMarkup(
      <DiffView diff={diff({ created: true, removed: 0 })} />,
    );
    expect(html).toContain("create");
    expect(html).not.toContain("replace");
  });

  it("names a write that would change nothing", () => {
    // Usually a model looping. An empty frame reads as a diff that failed.
    const html = renderToStaticMarkup(
      <DiffView diff={diff({ added: 0, removed: 0, hunks: [] })} />,
    );
    expect(html).toContain("exactly as it is");
  });

  it("announces lines it did not show", () => {
    // A cut that stays quiet reads as the whole change, which is the one thing
    // a permission prompt must never do.
    const html = renderToStaticMarkup(<DiffView diff={diff({ elided: 340 })} />);
    expect(html).toContain("340 more lines not shown");
  });
});

describe("permission dialog", () => {
  it("carries the diff when there is one", () => {
    const html = renderToStaticMarkup(
      <PermissionDialog request={request()} onDecide={() => {}} />,
    );
    expect(html).toContain("let x = 2;");
    // The one-line preview stays: it names the tool and the size, which the
    // diff does not.
    expect(html).toContain("Write src/widget.rs (2140 bytes)");
  });

  it("shows no diff frame for a call that has no before and after", () => {
    // A command line and a URL have none, and an empty frame would suggest the
    // diff was computed and came back blank.
    const html = renderToStaticMarkup(
      <PermissionDialog
        request={request({ tool: "run_command", effect: "execute", diff: null })}
        onDecide={() => {}}
      />,
    );
    expect(html).not.toContain("diff-body");
  });
});
