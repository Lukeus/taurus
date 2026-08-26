import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { FileDiff, PermissionRequest } from "../lib/api";
import { DiffView } from "./DiffView";
import { PermissionDialog } from "./PermissionDialog";

const diff = (patch: Partial<FileDiff> = {}): FileDiff => ({
  path: "src/widget.rs",
  created: false,
  deleted: false,
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
  offer_always: true,
  input: {},
  ...patch,
});

/**
 * The rendered text, with the markup taken back off.
 *
 * A diff line is no longer one text node: it is one span per run of syntax,
 * and another wherever the intra-line mark starts and stops. That is the
 * feature, and it means `toContain("let x = 1;")` against the raw markup now
 * asks whether the line happens to have been cut — which is a question about
 * the tokenizer, not about whether the line is on screen. So these read the
 * text the way a person does.
 */
const shown = (html: string) =>
  html
    .replace(/<[^>]*>/g, "")
    .replace(/&quot;/g, '"')
    .replace(/&#x27;/g, "'")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">");

describe("diff view", () => {
  it("shows both sides of a replaced line", () => {
    // The whole feature: the byte count says a file is about to be replaced
    // and nothing about what with.
    const html = renderToStaticMarkup(<DiffView diff={diff()} />);
    expect(shown(html)).toContain("let x = 1;");
    expect(shown(html)).toContain("let x = 2;");
  });

  it("colours the text by the language the file is in", () => {
    // The path is the only thing that says what the file is, and it is already
    // on the dialog. A change to a string should not have to be told apart
    // from a change to a name by reading both.
    const html = renderToStaticMarkup(<DiffView diff={diff()} />);
    expect(html).toContain('class="ink-keyword"');
    // And a file whose extension names no language still renders its text.
    const plain = renderToStaticMarkup(<DiffView diff={diff({ path: "NOTES" })} />);
    expect(shown(plain)).toContain("let x = 1;");
    expect(plain).not.toContain("ink-keyword");
  });

  it("marks the characters that actually differ inside a replaced line", () => {
    const html = renderToStaticMarkup(<DiffView diff={diff()} />);
    // `1` and `2`, and nothing else on those lines — the two sides share
    // everything up to the digit.
    const marked = [...html.matchAll(/ink-changed">([^<]*)</g)].map((m) => m[1]);
    expect(marked).toEqual(["1", "2"]);
  });

  it("marks nothing when a removal and an addition are not one line rewritten", () => {
    // Two removals answered by one addition: lines went as well as changed,
    // and pairing them by position would mark the difference between lines
    // that have nothing to do with each other.
    const html = renderToStaticMarkup(
      <DiffView
        diff={diff({
          hunks: [
            {
              lines: [
                { kind: "removed", text: "    let x = 1;", old_line: 2, new_line: null },
                { kind: "removed", text: "    let y = 2;", old_line: 3, new_line: null },
                { kind: "added", text: "    let x = 3;", old_line: null, new_line: 2 },
              ],
            },
          ],
        })}
      />,
    );
    expect(html).not.toContain("ink-changed");
    expect(shown(html)).toContain("let y = 2;");
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

  it("says delete rather than replace for a file that is gone", () => {
    // Only a recorded change can be this, and it has to be said: every line is
    // on the removed side either way, so an all-removed diff otherwise reads
    // identically to a file truncated to nothing.
    const html = renderToStaticMarkup(
      <DiffView diff={diff({ deleted: true, added: 0 })} />,
    );
    expect(html).toContain("delete");
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
    expect(shown(html)).toContain("let x = 2;");
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
