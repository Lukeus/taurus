// @vitest-environment jsdom
//
// Settings › Updates. What matters is that nothing reaches the network until
// the button is pressed, that an install is offered only when a check found
// something this copy can install, and that a failed install says why and
// leaves the offer standing.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { UpdateCheck, UpdateDownload } from "../../lib/api";

const checkForUpdate = vi.fn<() => Promise<UpdateCheck>>();
const installUpdate = vi.fn<(onProgress: (p: UpdateDownload) => void) => Promise<void>>();

vi.mock("../../lib/api", () => ({
  checkForUpdate: () => checkForUpdate(),
  installUpdate: (onProgress: (p: UpdateDownload) => void) => installUpdate(onProgress),
}));

import { UpdatesSection } from "./UpdatesSection";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

beforeEach(() => {
  checkForUpdate.mockReset();
  installUpdate.mockReset();
});

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => root.render(<UpdatesSection />));
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  const button = () => host.querySelector<HTMLButtonElement>("button")!;
  const press = async () => {
    await act(async () => button().click());
  };
  return { host, button, press };
}

const AVAILABLE: UpdateCheck = {
  current: "0.3.0",
  available: { version: "0.4.0", notes: "Signed updates.", date: "2026-10-25" },
};

describe("the updates section", () => {
  it("checks nothing until asked", () => {
    const { button } = mount();
    expect(checkForUpdate).not.toHaveBeenCalled();
    expect(button().textContent).toBe("Check for updates");
  });

  it("says so when the running version is the newest", async () => {
    checkForUpdate.mockResolvedValue({ current: "0.4.0" });
    const { host, button, press } = mount();
    await press();
    expect(host.textContent).toContain("0.4.0 · up to date");
    expect(button().textContent).toBe("Check for updates");
  });

  it("offers a found update, with its notes and what a restart ends", async () => {
    checkForUpdate.mockResolvedValue(AVAILABLE);
    const { host, button, press } = mount();
    await press();
    expect(host.textContent).toContain("0.4.0 is available, released 2026-10-25");
    expect(host.textContent).toContain("Signed updates.");
    expect(host.textContent).toContain("Restarting ends anything still running");
    expect(button().textContent).toBe("Install 0.4.0 and restart");
  });

  it("doesn't offer an install the build can't do", async () => {
    checkForUpdate.mockResolvedValue({ ...AVAILABLE, cannot_install: "This is a development build." });
    const { host, button, press } = mount();
    await press();
    expect(host.textContent).toContain("This is a development build.");
    expect(button().textContent).toBe("Check for updates");
  });

  it("shows progress, and a failed install keeps the offer and says why", async () => {
    checkForUpdate.mockResolvedValue(AVAILABLE);
    let fail: (e: string) => void = () => {};
    installUpdate.mockImplementation(
      (onProgress) =>
        new Promise((_, reject) => {
          onProgress({ done: 1_000_000, total: 4_000_000 });
          fail = reject;
        }),
    );
    const { host, button, press } = mount();
    await press();
    await press();
    expect(button().disabled).toBe(true);
    expect(host.querySelector<HTMLElement>(".plan-bar-fill")!.style.width).toBe("25%");

    await act(async () => fail("the update didn't install: signature mismatch"));
    expect(host.textContent).toContain("signature mismatch");
    expect(button().textContent).toBe("Install 0.4.0 and restart");
    expect(button().disabled).toBe(false);
  });
});
