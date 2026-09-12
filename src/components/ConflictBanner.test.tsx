import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { ConflictBanner } from "./ConflictBanner";

describe("the conflict banner", () => {
  it("says both versions still exist, and offers both", () => {
    const html = renderToStaticMarkup(
      <ConflictBanner
        className="notes-conflict"
        title="This note changed while you were working on it."
        onTakeTheirs={() => {}}
        onKeepMine={() => {}}
      />,
    );
    expect(html).toContain('class="notes-conflict" role="alert"');
    expect(html).toContain('class="notes-conflict-say"');
    expect(html).toContain("Your version is still here, unsaved.");
    expect(html).toContain("Take theirs");
    expect(html).toContain("Keep mine");
  });
});
