// @vitest-environment jsdom
//
// A link the system refuses to open. The click happens here and the refusal
// comes back later, so nothing on screen says so unless something is told.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, describe, expect, it, vi } from "vitest";

const openUrl = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: (href: string) => openUrl(href) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { Markdown } from "./Markdown";
import { FileLinks, LinkBase } from "../lib/links";
import { useStore } from "../state/store";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

async function click(text: string) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => createRoot(host).render(<Markdown text={text} streaming={false} />));
  await act(async () => host.querySelector("a")!.click());
}

beforeEach(() => {
  openUrl.mockReset();
  useStore.setState({ error: null });
});

it("opens a link, and says nothing when that works", async () => {
  openUrl.mockResolvedValue(undefined);
  await click("[docs](https://example.com/)");
  expect(openUrl).toHaveBeenCalledWith("https://example.com/");
  expect(useStore.getState().error).toBeNull();
});

it("says so in the banner when the link is refused", async () => {
  // A relative path: past the parser, which drops `file:` itself, and refused by
  // the capability's scope.
  openUrl.mockRejectedValue("Not allowed to open url src/main.rs");
  await click("[the file](src/main.rs)");
  expect(useStore.getState().error).toContain("src/main.rs");
  expect(useStore.getState().error).toContain("Not allowed");
});

describe("with a canvas to open into", () => {
  const openFile = vi.fn();
  const mount = async (text: string, base = "") => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    await act(async () =>
      createRoot(host).render(
        <FileLinks.Provider value={openFile}>
          <LinkBase.Provider value={base}>
            <Markdown text={text} streaming={false} />
          </LinkBase.Provider>
        </FileLinks.Provider>,
      ),
    );
    return host;
  };

  beforeEach(() => openFile.mockReset());

  it("opens a path in the canvas at the lines it names, not in the browser", async () => {
    const host = await mount("[App.tsx:42](src/App.tsx#L42-L51)");
    await act(async () => host.querySelector("a")!.click());
    expect(openFile).toHaveBeenCalledWith("src/App.tsx", { from: 42, to: 51 });
    expect(openUrl).not.toHaveBeenCalled();
    expect(host.querySelector("a")!.getAttribute("data-tip")).toBe("Open src/App.tsx, lines 42–51");
  });

  it("reads a relative link from the folder of the file it is in", async () => {
    const host = await mount("[config](./configuration.md) and [home](../README.md)", "docs");
    const [config, home] = [...host.querySelectorAll("a")];
    await act(async () => config.click());
    await act(async () => home.click());
    expect(openFile.mock.calls).toEqual([
      ["docs/configuration.md", null],
      ["README.md", null],
    ]);
  });

  it("still sends a web link to the browser", async () => {
    openUrl.mockResolvedValue(undefined);
    const host = await mount("[docs](https://example.com/)");
    await act(async () => host.querySelector("a")!.click());
    expect(openUrl).toHaveBeenCalledWith("https://example.com/");
    expect(openFile).not.toHaveBeenCalled();
  });

  it("scrolls to a heading on the same page", async () => {
    const host = await mount("[down](#the-ceiling)\n\n## What is not\n\n## The ceiling\n");
    const scrolled: string[] = [];
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this.textContent ?? "");
    };
    await act(async () => host.querySelector("a")!.click());
    expect(scrolled).toEqual(["The ceiling"]);
    expect(openFile).not.toHaveBeenCalled();
    expect(openUrl).not.toHaveBeenCalled();
  });
});
