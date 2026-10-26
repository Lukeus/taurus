// Startup, when the first read fails. Tauri is stood in for.
import { describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
const listened: string[] = [];
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string) => {
    listened.push(event);
    return Promise.resolve(() => {});
  },
}));

import { useStore } from "./store";

describe("starting up", () => {
  it("says the status couldn't be read, and still listens for the next one", async () => {
    // Unhandled, this left the window on the first-run screen with nothing
    // wired, so a later status push had nowhere to land.
    invoke.mockImplementation((command: string) =>
      command === "get_status"
        ? Promise.reject("the host panicked reading providers.json")
        : Promise.resolve([]),
    );
    await useStore.getState().init();

    expect(useStore.getState().error).toContain("couldn't read its status");
    expect(useStore.getState().error).toContain("panicked reading providers.json");
    expect(listened.length).toBeGreaterThan(0);
  });
});
