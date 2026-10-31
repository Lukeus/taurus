// @vitest-environment jsdom
//
// A link the system refuses to open. The click happens here and the refusal
// comes back later, so nothing on screen says so unless something is told.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, expect, it, vi } from "vitest";

const openUrl = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: (href: string) => openUrl(href) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { Markdown } from "./Markdown";
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
