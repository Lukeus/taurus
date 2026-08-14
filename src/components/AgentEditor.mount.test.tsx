// @vitest-environment jsdom
//
// Mounted, because everything interesting here is a control: the tool picker
// has three states, the character count gates the save, and Generate has to
// leave every field it filled in editable.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async (cmd: string) => {
  if (cmd === "list_tools") return ["read_file", "grep", "write_file"];
  if (cmd === "generate_agent")
    return {
      id: "d1",
      name: "diff-reviewer",
      description: "Reviews a diff for correctness bugs",
      prompt: "You review a diff and report defects you can point at a line for.",
      tools: ["read_file", "grep"],
      max_iterations: 25,
      rationale: "",
      replaces_existing: false,
    };
  return null;
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [string])),
  Channel: class {},
}));

const state = { session: { provider_id: "ollama", model: "qwen3.6:27b" } };
vi.mock("../state/store", () => ({
  useStore: (select: (s: typeof state) => unknown) => select(state),
}));

import { AgentEditor } from "./AgentEditor";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mount = async () => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => {
    createRoot(host).render(
      <AgentEditor onClose={() => {}} onSaved={() => {}} />,
    );
  });
  return host;
};

/** A selected tool chip reads "✓ read_file", so the tick is stripped first. */
const button = (host: HTMLElement, label: string) => {
  const found = [...host.querySelectorAll("button")].find((b) =>
    b.textContent?.replace("✓", "").trim().startsWith(label),
  );
  if (!found) throw new Error(`no ${label} button in: ${host.innerHTML}`);
  return found;
};

const press = async (host: HTMLElement, label: string) => {
  await act(async () => button(host, label).click());
};

/** Types into a controlled field the way React will notice. */
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
  return wrapper.querySelector("input, textarea") as
    | HTMLInputElement
    | HTMLTextAreaElement;
};

beforeEach(() => invoke.mockClear());
afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("the agent editor", () => {
  it("offers the tools this session actually has", async () => {
    // Not a compiled-in list: an agent scoped to a tool that is not here is
    // refused on save, and the picker is where that should be visible.
    const host = await mount();
    expect(invoke).toHaveBeenCalledWith("list_tools");
    const picks = [...host.querySelectorAll(".tool-chip.pick")].map((c) =>
      c.textContent?.trim(),
    );
    expect(picks).toEqual(["read_file", "grep", "write_file"]);
  });

  it("starts out inheriting, which is not the same as picking nothing", async () => {
    // `null` means inherit and `[]` means no tools at all. The checkbox is the
    // only thing distinguishing them, so it has to lead.
    const host = await mount();
    const inherit = host.querySelector(
      ".inherit input",
    ) as HTMLInputElement;
    expect(inherit.checked).toBe(true);
    for (const chip of host.querySelectorAll(".tool-chip.pick")) {
      expect((chip as HTMLButtonElement).disabled).toBe(true);
    }
  });

  it("lets a tool be picked once inheriting is turned off", async () => {
    const host = await mount();
    const inherit = host.querySelector(".inherit input") as HTMLInputElement;
    await act(async () => inherit.click());
    await press(host, "read_file");
    expect(button(host, "read_file").className).toContain("on");
  });

  it("keeps Save unavailable until the file would actually load", async () => {
    // Name, description and system prompt are all required by the loader. A
    // save that reaches Rust only to be rejected is a worse way to learn that.
    const host = await mount();
    const save = button(host, "Save agent") as HTMLButtonElement;
    expect(save.disabled).toBe(true);

    await type(field(host, "Name"), "diff-reviewer");
    await type(field(host, "Description"), "Reviews a diff");
    expect((button(host, "Save agent") as HTMLButtonElement).disabled).toBe(true);

    await type(field(host, "System prompt"), "You review a diff.");
    expect((button(host, "Save agent") as HTMLButtonElement).disabled).toBe(false);
  });

  it("refuses to save a description the roster would reject", async () => {
    // 200 characters is the loader's limit, and every one of them is paid for
    // on every request. The count turns red and Save goes away.
    const host = await mount();
    await type(field(host, "Name"), "diff-reviewer");
    await type(field(host, "System prompt"), "You review a diff.");
    await type(field(host, "Description"), "x".repeat(201));

    expect((button(host, "Save agent") as HTMLButtonElement).disabled).toBe(true);
    expect(host.querySelector(".hint.over")?.textContent).toContain("201 / 200");
  });

  it("fills every field from a draft and leaves them all editable", async () => {
    // The whole reason this is an editor with a draft button rather than a
    // wizard: what comes back is a starting point.
    const host = await mount();
    const ask = host.querySelector(".agent-generate textarea") as HTMLTextAreaElement;
    await type(ask, "reviews a diff for correctness bugs");
    await press(host, "Generate");

    expect(field(host, "Name").value).toBe("diff-reviewer");
    expect(field(host, "Max iterations").value).toBe("25");
    expect(button(host, "read_file").className).toContain("on");

    await type(field(host, "Name"), "my-reviewer");
    expect(field(host, "Name").value).toBe("my-reviewer");
  });

  it("will not draft from an empty description", async () => {
    const host = await mount();
    expect((button(host, "Generate") as HTMLButtonElement).disabled).toBe(true);
  });

  it("saves to this project by default, and says which it will be", async () => {
    // The two destinations differ in who gets the agent, and the header tag
    // tracks the choice so it is visible without scrolling to the footer.
    const host = await mount();
    expect(host.querySelector(".tag")?.textContent).toBe("project");
    await press(host, "all projects");
    expect(host.querySelector(".tag")?.textContent).toBe("all projects");
  });
});
