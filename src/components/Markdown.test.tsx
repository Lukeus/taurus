import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// The opener plugin reaches into Tauri internals that do not exist outside the
// webview; only the click handler uses it, and these tests never click.
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const { Markdown } = await import("./Markdown");

/** Renders as the transcript would once a turn has finished. */
const render = (text: string) =>
  renderToStaticMarkup(<Markdown text={text} streaming={false} />);

/** Renders mid-stream, when constructs may still be unclosed. */
const renderStreaming = (text: string) =>
  renderToStaticMarkup(<Markdown text={text} streaming />);

/** The rendered text, with the markup taken back off. See the note below. */
const shown = (html: string) =>
  html.replace(/<[^>]*>/g, "").replace(/&quot;/g, '"').replace(/&#x27;/g, "'");

describe("markdown rendering", () => {
  it("renders emphasis rather than showing the asterisks", () => {
    const html = render("Done. **README.md** is updated.");
    expect(html).toContain("<strong>README.md</strong>");
    expect(html).not.toContain("**");
  });

  it("renders bullets as a list", () => {
    const html = render("- first\n- second\n");
    expect(html).toContain("<ul>");
    expect(html).toContain("<li>first</li>");
  });

  it("renders inline code with its own styling hook", () => {
    const html = render("Run `cargo test` now.");
    expect(html).toContain('class="md-inline-code"');
    expect(html).toContain("cargo test");
  });

  it("renders a fenced block with its language and a copy button", () => {
    const html = render('```rust\nfn main() {}\n```');
    expect(html).toContain('class="md-code"');
    expect(html).toContain("rust");
    expect(html).toContain("copy");
    // Read as text: the body is now one span per run of syntax, so asking the
    // markup whether it holds the line uninterrupted asks about the tokenizer
    // rather than about what is on the page. That `rust` was coloured at all
    // is `ink.test.ts`'s business.
    expect(shown(html)).toContain("fn main() {}");
  });

  it("renders GFM tables", () => {
    const html = render("| a | b |\n| - | - |\n| 1 | 2 |\n");
    expect(html).toContain("<table>");
    expect(html).toContain("<th>a</th>");
  });

  it("renders headings", () => {
    expect(render("## Summary\n")).toContain("<h2>Summary</h2>");
  });

  it("renders links as anchors", () => {
    const html = render("see [the docs](https://example.com)");
    expect(html).toContain('href="https://example.com"');
  });

  // The model's output is not trusted markup. `react-markdown` drops HTML
  // unless `rehype-raw` is added, and it deliberately is not — so tags arrive
  // escaped, as visible text, and no live element is ever created.
  it("escapes raw HTML from model output instead of creating elements", () => {
    const html = render('<img src=x onerror="alert(1)"> and <b>bold</b>');
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<b>bold</b>");
    // The attribute survives only as escaped text, which cannot fire.
    expect(html).toContain("&lt;img");
    expect(html).toContain("onerror=&quot;");
  });

  it("escapes script tags in model output", () => {
    const html = render("<script>alert(1)</script>");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
  });

  describe("partial input while streaming", () => {
    // Every prefix of a real answer arrives on its own render pass, so none of
    // them may throw.
    const answer = [
      "Here is what I found.\n\n",
      "## Summary\n\n",
      "- **README.md** — the service\n",
      "- `CHANGELOG.md` — history\n\n",
      "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n",
      "See [docs](https://example.com).\n",
    ].join("");

    it("survives every prefix of a complete answer", () => {
      for (let i = 0; i <= answer.length; i++) {
        expect(() => renderStreaming(answer.slice(0, i))).not.toThrow();
      }
    });

    it("renders an unterminated code fence as a code block", () => {
      const html = renderStreaming("```rust\nfn main(");
      expect(html).toContain('class="md-code"');
      expect(shown(html)).toContain("fn main(");
    });

    it("leaves an unterminated emphasis marker as text", () => {
      expect(() => renderStreaming("this is **bol")).not.toThrow();
      expect(renderStreaming("this is **bol")).toContain("bol");
    });

    it("renders an incomplete table without dropping the rows", () => {
      const html = renderStreaming("| a | b |\n| - |");
      expect(html).toContain("a");
    });

    it("renders nothing for empty text", () => {
      expect(renderStreaming("")).toBe('<div class="markdown"></div>');
    });
  });
});
