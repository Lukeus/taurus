import { describe, expect, it } from "vitest";

import { classify, folderOf, slug } from "./links";

const W = "/Users/me/code/taurus";

describe("a link in rendered Markdown", () => {
  it("goes to the browser when it has a scheme", () => {
    expect(classify("https://example.com/a#b", "", W)).toEqual({
      kind: "web",
      href: "https://example.com/a#b",
    });
    expect(classify("mailto:me@example.com", "", W)?.kind).toBe("web");
  });

  it("is a workspace file when it is a path", () => {
    expect(classify("docs/retries.md", "", W)).toEqual({
      kind: "file",
      path: "docs/retries.md",
      lines: null,
    });
  });

  it("is read from the folder of the file it is in", () => {
    // A README in docs/ linking to its neighbour and to the root's README.
    expect(classify("./configuration.md", "docs", W)).toMatchObject({ path: "docs/configuration.md" });
    expect(classify("../README.md", "docs", W)).toMatchObject({ path: "README.md" });
  });

  it("takes lines from the fragment editors write, and the suffix compilers print", () => {
    expect(classify("src/App.tsx#L42", "", W)).toMatchObject({ lines: { from: 42, to: 42 } });
    expect(classify("src/App.tsx#L42-L51", "", W)).toMatchObject({
      path: "src/App.tsx",
      lines: { from: 42, to: 51 },
    });
    expect(classify("src/lib.rs:7", "", W)).toMatchObject({ path: "src/lib.rs", lines: { from: 7, to: 7 } });
    expect(classify("src/lib.rs:7-9", "", W)).toMatchObject({ lines: { from: 7, to: 9 } });
  });

  it("keeps a fragment that is not lines off the path and out of the range", () => {
    expect(classify("docs/a.md#the-ceiling", "", W)).toEqual({
      kind: "file",
      path: "docs/a.md",
      lines: null,
    });
  });

  it("makes a path inside the workspace relative, and reads other rooted paths from the root", () => {
    expect(classify(`${W}/src/App.tsx#L3`, "docs", W)).toMatchObject({ path: "src/App.tsx" });
    expect(classify(`file://${W}/README.md`, "", W)).toMatchObject({ path: "README.md" });
    expect(classify("/docs/a.md", "src", W)).toMatchObject({ path: "docs/a.md" });
  });

  it("decodes what was escaped to be a URL", () => {
    expect(classify("docs/my%20notes.md", "", W)).toMatchObject({ path: "docs/my notes.md" });
  });

  it("keeps a climb out of the workspace for the host to refuse", () => {
    expect(classify("../../etc/hosts", "docs", W)).toMatchObject({ path: "../etc/hosts" });
  });

  it("is a heading on the same page when it is only a fragment", () => {
    expect(classify("#The-Ceiling", "", W)).toEqual({ kind: "heading", slug: "the-ceiling" });
    expect(classify("#", "", W)).toBeNull();
    expect(classify("", "", W)).toBeNull();
  });
});

describe("the anchors headings get", () => {
  it("are GitHub's", () => {
    expect(slug("What is retried")).toBe("what-is-retried");
    expect(slug("It is a split, not a screen")).toBe("it-is-a-split-not-a-screen");
    expect(slug("`open_file` and lines")).toBe("open_file-and-lines");
  });
});

describe("the folder a file's links are read from", () => {
  it("is everything before its name", () => {
    expect(folderOf("docs/guide/a.md")).toBe("docs/guide");
    expect(folderOf("README.md")).toBe("");
  });
});
