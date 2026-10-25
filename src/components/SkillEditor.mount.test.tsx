// @vitest-environment jsdom
//
// Mounted, because what matters here is controls: the trigger count gates the
// save, Generate fills fields that stay editable, a plugin you made is offered
// as a place to save, and a refused save says why without closing.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async (cmd: string, _args?: unknown): Promise<unknown> => {
  if (cmd === "list_plugins")
    return [
      { name: "kit", problems: [], shadowed: false },
      { name: "ops", problems: [], shadowed: false, source: { from: "https://x" } },
    ];
  if (cmd === "generate_skill")
    return {
      id: "",
      name: "cut-release",
      description: "Cuts a release",
      when_to_use: "When a release is asked for",
      body: "1. Run the tests.\n2. Tag the commit.",
      scripts: [],
      files: [],
      rationale: "",
      replaces_existing: false,
    };
  return "/somewhere/cut-release";
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [string, unknown])),
  Channel: class {},
}));

const state = { session: { provider_id: "ollama", model: "qwen3.6:27b" } };
vi.mock("../state/store", () => ({
  useStore: (select: (s: typeof state) => unknown) => select(state),
}));

import { SkillEditor, TRIGGER_LIMIT } from "./SkillEditor";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mount = async (props: Partial<Parameters<typeof SkillEditor>[0]> = {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const saved = vi.fn();
  await act(async () => {
    createRoot(host).render(<SkillEditor onClose={() => {}} onSaved={saved} {...props} />);
  });
  return { host, saved };
};

const button = (host: HTMLElement, label: string) => {
  const found = [...host.querySelectorAll("button")].find(
    (b) => b.textContent?.trim() === label,
  );
  if (!found) throw new Error(`no ${label} button in: ${host.innerHTML}`);
  return found;
};

const press = async (host: HTMLElement, label: string) => {
  await act(async () => button(host, label).click());
};

const type = async (element: HTMLTextAreaElement | HTMLInputElement, value: string) => {
  const proto =
    element instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")!.set!;
  await act(async () => {
    setter.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
  });
};

const field = (host: HTMLElement, label: string) => {
  const wrapper = [...host.querySelectorAll(".field")].find((f) =>
    f.querySelector(".micro")?.textContent?.includes(label),
  );
  if (!wrapper) throw new Error(`no "${label}" field in: ${host.innerHTML}`);
  return wrapper.querySelector("input, textarea") as HTMLInputElement | HTMLTextAreaElement;
};

const fill = async (host: HTMLElement) => {
  await type(field(host, "Name"), "cut-release");
  await type(field(host, "Description"), "Cuts a release");
  await type(field(host, "When to use it"), "When a release is asked for");
  await type(field(host, "Procedure"), "1. Run the tests.\n2. Tag the commit.");
};

beforeEach(() => invoke.mockClear());
afterEach(() => {
  document.body.innerHTML = "";
});

describe("the skill editor", () => {
  it("saves a new skill where it was asked to", async () => {
    const { host, saved } = await mount();
    await fill(host);
    await press(host, "all projects");
    await press(host, "Save skill");

    const call = invoke.mock.calls.find(([cmd]) => cmd === "create_skill");
    expect(call?.[1]).toMatchObject({
      draft: { name: "cut-release", when_to_use: "When a release is asked for", scripts: [] },
      target: "user",
      plugin: null,
    });
    expect(saved).toHaveBeenCalled();
  });

  it("offers a plugin you made, and not one fetched from somewhere", async () => {
    const { host } = await mount();
    expect(() => button(host, "kit")).not.toThrow();
    expect(() => button(host, "ops")).toThrow();

    await fill(host);
    await press(host, "kit");
    await press(host, "Save skill");
    const call = invoke.mock.calls.find(([cmd]) => cmd === "create_skill");
    expect(call?.[1]).toMatchObject({ plugin: "kit" });
  });

  it("won't save a trigger line longer than the loader reads", async () => {
    const { host } = await mount();
    await fill(host);
    await type(field(host, "When to use it"), "x".repeat(TRIGGER_LIMIT + 1));
    expect(button(host, "Save skill").disabled).toBe(true);
  });

  it("fills every field from Generate and leaves them editable", async () => {
    const { host } = await mount();
    await type(host.querySelector('textarea[aria-label="Describe what it should do"]')!, "releases");
    await press(host, "Generate");
    expect((field(host, "Name") as HTMLInputElement).value).toBe("cut-release");
    await type(field(host, "Name"), "ship-it");
    expect((field(host, "Name") as HTMLInputElement).value).toBe("ship-it");
  });

  it("says why a save was refused and stays open", async () => {
    invoke.mockImplementationOnce(async () => []); // list_plugins
    const { host, saved } = await mount();
    await fill(host);
    invoke.mockImplementationOnce(async () => {
      throw "a skill named \"cut-release\" is already at /x. Open that one to change it";
    });
    await press(host, "Save skill");
    expect(host.textContent).toContain("Open that one to change it");
    expect(saved).not.toHaveBeenCalled();
  });

  it("hands a proposal's edits to its own approval, without Generate or plugins", async () => {
    const submit = vi.fn(async () => {});
    const { host } = await mount({
      initial: {
        name: "cut-release",
        description: "Cuts a release",
        when_to_use: "When a release is asked for",
        body: "1. Run the tests.",
      },
      offerPlugins: false,
      onSubmit: submit,
    });
    expect(host.querySelector(".agent-generate")).toBeNull();
    expect(() => button(host, "kit")).toThrow();
    await press(host, "Save skill");
    expect(submit).toHaveBeenCalledWith(
      expect.objectContaining({ name: "cut-release" }),
      "project",
      undefined,
    );
    expect(invoke.mock.calls.some(([cmd]) => cmd === "create_skill")).toBe(false);
  });
});
