// What opening a conversation re-reads, and whether two lists wait on each
// other. Actions rather than the reducer, so Tauri is stood in for.
import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { useStore } from "./store";

const SESSION = {
  id: "s2",
  model: "m",
  provider_id: "local",
  native_tools: true,
  vision: false,
  context_length: 8192,
};

const asked = () => invoke.mock.calls.map(([command]) => command as string);

beforeEach(() => {
  invoke.mockReset();
  useStore.setState({ session: null, sessions: [], changed: [] });
});

describe("opening a conversation", () => {
  it("asks for its changed files, and not for the whole list", async () => {
    // The list arrives by event whenever a conversation in it moves. Asking for
    // it again opened and partly parsed every transcript in the workspace, on
    // every switch, to say what the rail already showed.
    invoke.mockImplementation((command: string) => {
      if (command === "resume_session") {
        return Promise.resolve({ ...SESSION, messages: [], switches: [] });
      }
      if (command === "list_checkpoints") {
        return Promise.resolve([{ turn: 1, files: ["a.rs"] }]);
      }
      return Promise.resolve([]);
    });

    await useStore.getState().resume("s2");

    await vi.waitFor(() => expect(useStore.getState().changed).toEqual(["a.rs"]));
    expect(asked()).not.toContain("list_sessions");
  });
});

describe("re-reading both lists", () => {
  it("asks for the changed files while the list is still being read", async () => {
    // Two requests to two files. Waiting for the first before asking the
    // second put the sum of both in front of the rail.
    useStore.setState({ session: SESSION as never });
    let answer: (value: unknown) => void = () => {};
    invoke.mockImplementation((command: string) =>
      command === "list_sessions"
        ? new Promise((resolve) => {
            answer = resolve;
          })
        : Promise.resolve([]),
    );

    const reloading = useStore.getState().reload();
    await Promise.resolve();

    expect(asked()).toContain("list_checkpoints");
    answer([]);
    await reloading;
  });
});
