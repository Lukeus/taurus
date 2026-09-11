import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { describe, expect, it, vi } from "vitest";

// The opener plugin reaches into Tauri internals that do not exist outside the
// webview; only the click handler uses it, and these tests never click.
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const { Markdown, blocks } = await import("./Markdown");

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

describe("cutting an answer into stretches", () => {
  const cuts = (text: string) => blocks(text).map((b) => b.text);

  it("cuts before a paragraph that follows a blank line", () => {
    expect(cuts("One.\n\nTwo.\n\nThree")).toEqual(["One.\n\n", "Two.\n\n", "Three"]);
  });

  it("gives back every character, each stretch starting where the last ended", () => {
    const text = "# Head\n\nprose\n\n```rust\nfn a() {}\n```\n\n- x\n- y\n\ntail";
    const pieces = blocks(text);
    expect(pieces.map((b) => b.text).join("")).toBe(text);
    pieces.forEach((b) => expect(text.startsWith(b.text, b.at)).toBe(true));
  });

  it("never cuts inside a fence, blank lines and all", () => {
    expect(cuts("```\na\n\nb\n```\n\nc")).toEqual(["```\na\n\nb\n```\n\n", "c"]);
    // Closed only by a run as long as the one that opened it.
    expect(cuts("````\n```\n\nx\n````\n\ny")).toEqual(["````\n```\n\nx\n````\n\n", "y"]);
    expect(cuts("~~~\n\n```\n\n~~~\n\nz")).toEqual(["~~~\n\n```\n\n~~~\n\n", "z"]);
  });

  it("does not cut inside a fence that has not closed yet", () => {
    expect(cuts("Intro.\n\n```rust\nfn a() {\n\n    b();")).toEqual([
      "Intro.\n\n",
      "```rust\nfn a() {\n\n    b();",
    ]);
  });

  it("does not cut before what could continue a list, or an indented line", () => {
    expect(cuts("- a\n\n- b")).toHaveLength(1);
    expect(cuts("1. a\n\n2. b")).toHaveLength(1);
    expect(cuts("- a\n\n  still a")).toHaveLength(1);
    expect(cuts("text\n\n    code")).toHaveLength(1);
  });

  it("leaves whole what a definition or a raw block could reach across", () => {
    expect(cuts("see [x]\n\n[x]: https://example.com")).toHaveLength(1);
    expect(cuts("a[^1]\n\n[^1]: note")).toHaveLength(1);
    expect(cuts("<!--\n\nhidden\n\n-->\n\nafter")).toHaveLength(1);
  });

  /*
   * The property the cutting is allowed to rely on and nothing else: parsed
   * apart, the pieces draw what the whole would have. Compared with the blank
   * text between block elements taken out, since the whole document separates
   * its blocks with a newline the pieces do not have — whitespace between
   * blocks, which draws nothing.
   */
  it("draws what parsing the whole would have", () => {
    const draw = (text: string) =>
      renderToStaticMarkup(<ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>)
        .replace(/>\n+</g, "><");
    const documents = [
      "Here is what I found.\n\n## Summary\n\n- **a** — one\n- `b` — two\n\n```rust\nfn main() {\n\n    x();\n}\n```\n\nSee [docs](https://example.com).\n",
      "- loose\n\n- list\n\nAfter it.\n",
      "1. one\n\n2. two\n\n3. three\n",
      "> quoted\n\n> again\n\nplain\n",
      "| a | b |\n| - | - |\n| 1 | 2 |\n\nUnder the table.\n",
      "Title\n---\n\n***\n\n    indented code\n\n    more\n\ntext\n",
      "- item\n\n  continued in the item\n\nout of it\n",
      "~~~md\n# not a heading\n\n~~~\n\nafter\n",
      "see [the docs][d]\n\n[d]: https://example.com\n",
    ];
    for (const text of documents) {
      expect(blocks(text).map((b) => draw(b.text)).join(""), text).toBe(draw(text));
    }
  });
});

describe("a mermaid fence", () => {
  it("draws a flowchart rather than printing its source", () => {
    const html = render(
      "```mermaid\nflowchart LR\n  a[Client] --> b[API]\n```",
    );
    expect(html).toContain('role="img"');
    expect(html).toContain('class="flow"');
    expect(shown(html)).toContain("Client");
    // And the description a screen reader gets, since the picture says nothing.
    expect(html).toContain("Flow diagram");
  });

  it("draws a sequence diagram", () => {
    const html = render(
      "```mermaid\nsequenceDiagram\n  c->>api: POST /orders\n  api-->>c: 201\n```",
    );
    expect(html).toContain('class="sequence"');
    expect(html).toContain("Sequence diagram");
    expect(shown(html)).toContain("POST /orders");
  });

  it("shows the source and says why, for a diagram it cannot draw", () => {
    // Not an error state. A fence is text somebody wrote and the text is never
    // wrong, so the source stays and a sentence goes over it.
    const html = render("```mermaid\ngantt\n  title A schedule\n```");
    expect(html).not.toContain("<svg");
    expect(html).toContain("md-mermaid-note");
    expect(shown(html)).toContain("Gantt charts");
    expect(shown(html)).toContain("title A schedule");
  });

  it("says what it left out of a diagram it did draw", () => {
    const html = render(
      "```mermaid\ngraph TD\n  a --> b\n```",
    );
    expect(html).toContain('class="flow"');
    expect(shown(html)).toContain("Laid out left to right rather than top to bottom.");
  });

  it("leaves a half-written fence as source while the turn streams", () => {
    // A fence three lines into being written parses as a refusal, and flickering
    // through one on the way to a picture is worse than waiting for the text to
    // stop moving.
    const half = "```mermaid\nflowchart LR\n  a[Client] --> ";
    expect(renderStreaming(half)).not.toContain("<svg");
    expect(renderStreaming(half)).not.toContain("md-mermaid-note");
    // The same text, once it is finished and no longer streaming.
    expect(render(`${half}b[API]\n\`\`\``)).toContain('class="flow"');
  });

  it("leaves every other fence alone", () => {
    const html = render("```rust\nfn main() {}\n```");
    expect(html).not.toContain("<svg");
    expect(html).toContain("md-code-lang");
  });
});
