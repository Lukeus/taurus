// @vitest-environment jsdom
//
// The list says where each credential goes and who uses it without showing a
// token; adding sends the token once; editing leaves it alone unless a new
// one is pasted; removing asks first.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));

import { CredentialsSection, reach, splitHosts } from "./CredentialsSection";
import type { CredentialView } from "../../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let list: CredentialView[];
let keychain: boolean;

beforeEach(() => {
  keychain = true;
  list = [
    { name: "linear", hosts: ["mcp.linear.app"], stored: true, used_by: ["linear"] },
    { name: "github", hosts: [], stored: false, used_by: [] },
  ];
  invoke.mockImplementation((command: string) => {
    if (command === "list_credentials") return Promise.resolve(list);
    if (command === "keychain_available") return Promise.resolve(keychain);
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
  await act(async () => createRoot(host).render(<CredentialsSection />));
  return host;
};

const button = (within: HTMLElement, label: string) => {
  const found = [...within.querySelectorAll("button")].find((b) => b.textContent === label);
  if (!found) throw new Error(`no "${label}" in: ${within.innerHTML}`);
  return found;
};

const card = (host: HTMLElement, name: string) =>
  [...host.querySelectorAll<HTMLElement>(".credential-card")].find(
    (c) => c.querySelector(".card-title")?.textContent === name,
  )!;

const type = async (host: HTMLElement, label: string, value: string) => {
  const input = host.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    set.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
};

const press = (el: HTMLElement) => act(async () => el.click());
const calls = (command: string) => invoke.mock.calls.filter(([c]) => c === command);

describe("saved credentials", () => {
  it("says where each goes and who uses it", async () => {
    const host = await mount();
    expect(card(host, "linear").textContent).toContain("Only mcp.linear.app.");
    expect(card(host, "linear").textContent).toContain("Used by linear.");
    expect(card(host, "github").textContent).toContain("Any host, from your own mcp.json only");
    expect(card(host, "github").textContent).toContain("No server uses it.");
  });

  it("says so when the keychain has lost one", async () => {
    const host = await mount();
    expect(card(host, "github").textContent).toContain("paste the token again");
    expect(card(host, "linear").textContent).not.toContain("paste the token again");
  });

  it("adds one with its token and hosts, split on commas or spaces", async () => {
    const host = await mount();
    await press(button(host, "+ Add a credential"));
    expect(button(host, "Save").disabled).toBe(true);
    await type(host, "Credential name", "sentry");
    await type(host, "Token", "sntrys_abc");
    await type(host, "Hosts", "mcp.sentry.dev, *.sentry.io  example.com");
    await press(button(host, "Save"));

    expect(calls("save_credential")[0][1]).toEqual({
      name: "sentry",
      hosts: ["mcp.sentry.dev", "*.sentry.io", "example.com"],
      value: "sntrys_abc",
    });
    expect(host.querySelector('input[aria-label="Token"]')).toBeNull();
  });

  it("edits the hosts without touching the token unless a new one is pasted", async () => {
    const host = await mount();
    await press(button(card(host, "linear"), "Edit"));
    const name = host.querySelector<HTMLInputElement>('input[aria-label="Credential name"]')!;
    expect(name.disabled).toBe(true);
    expect(host.querySelector<HTMLInputElement>('input[aria-label="Token"]')!.value).toBe("");
    await type(host, "Hosts", "mcp.linear.app, api.linear.app");
    await press(button(host, "Save"));
    expect(calls("save_credential")[0][1]).toEqual({
      name: "linear",
      hosts: ["mcp.linear.app", "api.linear.app"],
      value: null,
    });
  });

  it("asks before removing one, naming what will stop working", async () => {
    const host = await mount();
    await press(button(card(host, "linear"), "Remove"));
    expect(calls("remove_credential")).toHaveLength(0);
    expect(card(host, "linear").textContent).toContain("linear will stop connecting");
    await press(button(card(host, "linear"), "Remove linear"));
    expect(calls("remove_credential")[0][1]).toEqual({ name: "linear" });
  });

  it("explains, and offers nothing to fill in, without a keychain", async () => {
    keychain = false;
    list = [];
    const host = await mount();
    expect(host.textContent).toContain("no keychain");
    expect([...host.querySelectorAll("button")].some((b) => b.textContent?.includes("Add"))).toBe(
      false,
    );
  });

  it("shows a refusal from the backend in the form", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "list_credentials") return Promise.resolve(list);
      if (command === "keychain_available") return Promise.resolve(true);
      if (command === "save_credential") return Promise.reject("not a host is not a host.");
      return Promise.resolve();
    });
    const host = await mount();
    await press(button(host, "+ Add a credential"));
    await type(host, "Credential name", "x");
    await type(host, "Token", "t");
    await press(button(host, "Save"));
    expect(host.textContent).toContain("not a host is not a host.");
    expect(host.querySelector('input[aria-label="Token"]')).not.toBeNull();
  });
});

describe("the helpers", () => {
  it("splits hosts on commas, spaces and new lines", () => {
    expect(splitHosts(" a.com,b.com  c.com\n\n")).toEqual(["a.com", "b.com", "c.com"]);
  });

  it("says what an empty list means", () => {
    expect(reach([])).toContain("your own mcp.json");
  });
});
