// @vitest-environment jsdom
//
// The name saves on blur, the switch can't be turned on without one, and a
// refused name stays on screen beside the reason.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

const state: { status: unknown } = { status: null };
vi.mock("../../state/store", () => ({
  useStore: (select: (s: typeof state) => unknown) => select(state),
}));

import { ProfileTab } from "./ProfileTab";
import type { Profile } from "../../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const PATH = "/home/ada/.taurus/profiles/default/profile.json";
let stored: Profile;

beforeEach(() => {
  state.status = { problems: [] };
  stored = { name: "", introduce: false };
  invoke.mockImplementation((command: string, args?: { profile: Profile }) => {
    if (command === "get_profile") {
      return Promise.resolve({ id: "default", profile: stored, path: PATH });
    }
    if (command === "save_profile") {
      const next = args!.profile;
      if (next.name.length > 80) {
        return Promise.reject("A name can be up to 80 characters. This one is 81.");
      }
      stored = { ...next, name: next.name.trim() };
      return Promise.resolve({ id: "default", profile: stored, path: PATH });
    }
    return Promise.resolve();
  });
});

afterEach(() => {
  document.body.innerHTML = "";
  invoke.mockReset();
});

const mount = async () => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  await act(async () => createRoot(host).render(<ProfileTab />));
  return host;
};

const nameBox = (host: HTMLElement) =>
  host.querySelector<HTMLInputElement>('input[aria-label="Your name"]')!;
const introduce = (host: HTMLElement) =>
  host.querySelector<HTMLInputElement>('input[type="checkbox"]')!;

/** Types into a controlled input the way React hears it. */
const type = async (input: HTMLInputElement, value: string) => {
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    set.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
};

const blur = async (input: HTMLInputElement) => {
  await act(async () => {
    input.focus();
    input.blur();
  });
};

const saves = () => invoke.mock.calls.filter(([command]) => command === "save_profile");

describe("the profile tab", () => {
  it("says where the profile is kept", async () => {
    const host = await mount();
    expect(host.textContent).toContain(PATH);
  });

  it("won't offer to introduce somebody who has no name", async () => {
    const host = await mount();
    expect(introduce(host).disabled).toBe(true);
    expect(host.textContent).toContain("Add a name first.");
  });

  it("saves the name on blur, trimmed, and then offers the switch", async () => {
    const host = await mount();
    await type(nameBox(host), "  Ada ");
    await blur(nameBox(host));

    expect(saves()).toHaveLength(1);
    expect(saves()[0][1]).toEqual({ profile: { name: "  Ada ", introduce: false } });
    expect(nameBox(host).value).toBe("Ada");
    expect(introduce(host).disabled).toBe(false);
  });

  it("doesn't save a name that didn't change", async () => {
    stored = { name: "Ada", introduce: false };
    const host = await mount();
    await blur(nameBox(host));
    expect(saves()).toHaveLength(0);
  });

  it("sends the name along with the switch, so neither undoes the other", async () => {
    stored = { name: "Ada", introduce: false };
    const host = await mount();
    await act(async () => introduce(host).click());
    expect(saves()[0][1]).toEqual({ profile: { name: "Ada", introduce: true } });
    expect(introduce(host).checked).toBe(true);
  });

  it("leaves a refused name on screen beside the reason", async () => {
    const host = await mount();
    const long = "a".repeat(81);
    await type(nameBox(host), long);
    await blur(nameBox(host));
    expect(nameBox(host).value).toBe(long);
    expect(host.textContent).toContain("up to 80 characters");
  });

  it("shows what was wrong with the file", async () => {
    state.status = {
      problems: [{ source: "profile", message: "profile.json isn't valid: trailing comma" }],
    };
    const host = await mount();
    expect(host.textContent).toContain("trailing comma");
  });
});
