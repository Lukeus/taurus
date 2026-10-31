// @vitest-environment jsdom
//
// The dock's failure paths: a shell that never starts, and a message that
// outlives what it was about. The emulator is stood in for; what's checked is
// the chrome around it.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {
    onmessage: ((value: unknown) => void) | null = null;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

const typed: ((data: string) => void)[] = [];
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    rows = 24;
    cols = 80;
    options: Record<string, unknown> = {};
    parser = { registerOscHandler: () => ({ dispose() {} }) };
    loadAddon() {}
    open() {}
    onData(handler: (data: string) => void) {
      typed.push(handler);
      return { dispose() {} };
    }
    onResize() {
      return { dispose() {} };
    }
    attachCustomKeyEventHandler() {}
    registerMarker() {
      return undefined;
    }
    registerDecoration() {
      return undefined;
    }
    write(_data: unknown, done?: () => void) {
      done?.();
    }
    focus() {}
    scrollToLine() {}
    dispose() {}
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
const webgl = vi.hoisted(() => ({ fails: false, lost: [] as (() => void)[], disposed: 0 }));
vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    constructor() {
      if (webgl.fails) throw new Error("no WebGL here");
    }
    onContextLoss(handler: () => void) {
      webgl.lost.push(handler);
    }
    dispose() {
      webgl.disposed += 1;
    }
  },
}));
vi.mock("../vendor.css", () => ({}));

(globalThis as { ResizeObserver?: unknown }).ResizeObserver = class {
  observe() {}
  disconnect() {}
};
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

import { TerminalDock } from "./TerminalDock";

afterEach(() => {
  document.body.innerHTML = "";
  invoke.mockReset();
  typed.length = 0;
  webgl.fails = false;
  webgl.lost.length = 0;
  webgl.disposed = 0;
});

async function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <TerminalDock
        workspace="/src/project"
        jobs={[]}
        watching={null}
        output={null as never}
        onWatch={() => {}}
        onStop={() => Promise.resolve()}
        onClose={() => {}}
        onAsk={() => {}}
      />,
    );
  });
  const button = (label: string) =>
    [...host.querySelectorAll("button")].find(
      (b) => b.textContent === label || b.getAttribute("aria-label") === label,
    );
  return { host, root, button };
}

describe("the renderer", () => {
  const opens = () =>
    invoke.mockImplementation((command: string) =>
      command === "terminal_open" ? Promise.resolve("t1") : Promise.resolve(),
    );

  it("still opens the shell where WebGL isn't to be had", async () => {
    webgl.fails = true;
    opens();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const dock = await mount();
    expect(invoke).toHaveBeenCalledWith("terminal_open", expect.anything());
    expect(dock.host.textContent).not.toContain("couldn't start");
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });

  it("lets go of WebGL when its context is lost", async () => {
    opens();
    await mount();
    expect(webgl.lost).toHaveLength(1);
    webgl.lost[0]();
    expect(webgl.disposed).toBe(1);
  });
});

describe("a shell that never starts", () => {
  it("says so, offers Restart, and stops queueing what's typed", async () => {
    invoke.mockImplementation((command: string) =>
      command === "terminal_open"
        ? Promise.reject("could not run /bin/fish: No such file or directory")
        : Promise.resolve(),
    );
    const dock = await mount();
    expect(dock.host.textContent).toContain("The shell couldn't start");
    expect(dock.host.textContent).toContain("shell didn't start");
    expect(dock.button("Restart")).toBeTruthy();

    typed.forEach((handler) => handler("ls\r"));
    expect(invoke).not.toHaveBeenCalledWith("terminal_write", expect.anything());

    // Restart tries again, and clears the message about the last attempt.
    invoke.mockImplementation((command: string) =>
      command === "terminal_open" ? Promise.resolve("t2") : Promise.resolve(),
    );
    await act(async () => dock.button("Restart")!.click());
    expect(dock.host.textContent).not.toContain("couldn't start");
    expect(dock.button("Restart")).toBeUndefined();
    act(() => dock.root.unmount());
  });
});

describe("a message about the shell", () => {
  it("can be dismissed while the shell is still running", async () => {
    let written = 0;
    invoke.mockImplementation((command: string) => {
      if (command === "terminal_open") return Promise.resolve("t1");
      if (command === "terminal_write") {
        written += 1;
        return Promise.reject("the terminal is not accepting input");
      }
      return Promise.resolve();
    });
    const dock = await mount();
    await act(async () => typed.forEach((handler) => handler("x")));
    expect(written).toBe(1);
    expect(dock.host.textContent).toContain("The terminal is not accepting input");

    await act(async () => dock.button("Dismiss")!.click());
    expect(dock.host.querySelector(".dock-problem")).toBeNull();
    act(() => dock.root.unmount());
  });
});
