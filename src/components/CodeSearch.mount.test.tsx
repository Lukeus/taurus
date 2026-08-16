// @vitest-environment jsdom
//
// The section exists to make a forty-four-second job something you can start,
// watch, and stop. All three are behaviour rather than markup, so a string
// render would prove none of them.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(() => Promise.resolve("Index is current: 412 files."));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {
    onmessage: ((value: unknown) => void) | null = null;
  },
}));

const refresh = vi.fn(() => Promise.resolve());
vi.mock("../state/store", () => ({
  useStore: (select: (s: { refresh: () => Promise<void> }) => unknown) =>
    select({ refresh }),
}));

import { CodeSearch } from "./Settings";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mount = async (model: string) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => {
    createRoot(host).render(<CodeSearch model={model} />);
  });
  return host;
};

const button = (host: HTMLElement, label: string) =>
  [...host.querySelectorAll("button")].find((b) => b.textContent === label);

beforeEach(() => {
  invoke.mockClear();
  refresh.mockClear();
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("building the code index from settings", () => {
  it("offers no build until there is a model to build with", async () => {
    // A button that can only fail is worse than no button. With no embedding
    // model the whole feature is off, and the field is what turns it on.
    const host = await mount("");
    expect(button(host, "Build index now")).toBeUndefined();
    expect(host.textContent).toMatch(/nomic-embed-text/);
  });

  it("offers the build once a model is named", async () => {
    const host = await mount("nomic-embed-text");
    expect(button(host, "Build index now")).toBeTruthy();
  });

  it("writes the model on blur rather than on every keystroke", async () => {
    // Each write reloads the registry, so saving per character would rebuild
    // the tool set once per letter of "nomic-embed-text".
    const host = await mount("");
    const field = host.querySelector("input")!;

    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )!.set!;
      setter.call(field, "nomic-embed-text");
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(invoke).not.toHaveBeenCalled();

    await act(async () => {
      field.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
    });
    expect(invoke).toHaveBeenCalledWith("set_embedding_model", {
      model: "nomic-embed-text",
    });
  });

  it("does not rewrite the model when the field was not changed", async () => {
    const host = await mount("nomic-embed-text");
    await act(async () => {
      host.querySelector("input")!.dispatchEvent(
        new FocusEvent("focusout", { bubbles: true }),
      );
    });
    expect(invoke).not.toHaveBeenCalled();
  });

  it("turns into a Stop on the click, not on the first batch", async () => {
    // A cold Ollama takes seconds to answer the first batch. A button that
    // still says "Build index now" for all of them reads as a click that
    // missed, and the second click starts a second build.
    let finish: (value: string) => void = () => {};
    invoke.mockImplementationOnce(
      () => new Promise<string>((resolve) => (finish = resolve)),
    );

    const host = await mount("nomic-embed-text");
    await act(async () => {
      button(host, "Build index now")!.click();
    });

    expect(button(host, "Stop")).toBeTruthy();
    expect(button(host, "Build index now")).toBeUndefined();
    expect(host.textContent).toMatch(/reading the workspace/);

    await act(async () => {
      finish("Indexed 412 files (1180 chunks); 0 already current.");
    });
    expect(button(host, "Build index now")).toBeTruthy();
    expect(host.textContent).toMatch(/Indexed 412 files/);
  });

  it("stops a running build through its own command", async () => {
    invoke.mockImplementationOnce(() => new Promise<string>(() => {}));
    const host = await mount("nomic-embed-text");
    await act(async () => {
      button(host, "Build index now")!.click();
    });

    await act(async () => {
      button(host, "Stop")!.click();
    });
    expect(invoke).toHaveBeenCalledWith("stop_index_build");
  });

  it("reports a failure where the summary would have gone", async () => {
    // Most likely of all on a first run: an embedding model the server has not
    // pulled. Swallowing that leaves a button that appears to do nothing.
    invoke.mockImplementationOnce(() =>
      Promise.reject("model 'nomic-embed-text' not found"),
    );
    const host = await mount("nomic-embed-text");
    await act(async () => {
      button(host, "Build index now")!.click();
    });

    expect(host.textContent).toMatch(/not found/);
    expect(button(host, "Build index now")).toBeTruthy();
  });
});
