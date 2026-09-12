import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { InkLayer } from "./InkLayer";

describe("the painted layer", () => {
  it("ends with the newline a <pre> would otherwise swallow", () => {
    // Without it, text ending in a newline paints a line short and the caret
    // sits below its own text — found once in each of the three editors.
    const html = renderToStaticMarkup(
      <InkLayer runs={[{ text: "SELECT 1\n", kind: "keyword" }]} className="sql-ink" />,
    );
    expect(html).toBe(
      '<pre class="painted-ink sql-ink" aria-hidden="true"><span class="ink-keyword">SELECT 1\n</span>\n</pre>',
    );
  });

  it("puts what stands in for the rest of a long file on either side of the runs", () => {
    const html = renderToStaticMarkup(
      <InkLayer
        runs={[{ text: "fn main() {}", kind: "plain" }]}
        className="doc-ink"
        before={<div data-above="" />}
        after={<div data-below="" />}
      />,
    );
    expect(html.indexOf("data-above")).toBeLessThan(html.indexOf("fn main"));
    expect(html.indexOf("fn main")).toBeLessThan(html.indexOf("data-below"));
  });
});
