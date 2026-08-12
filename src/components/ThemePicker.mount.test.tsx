// @vitest-environment jsdom
//
// Mounted rather than rendered to a string, because the question here is what a
// click does — and what it does before the answer comes back from Rust.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(() => Promise.resolve([]));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));

import { ThemePicker } from "./Settings";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mount = (node: React.ReactNode) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  act(() => createRoot(host).render(node));
  return host;
};

const click = (host: HTMLElement, label: string) => {
  const button = [...host.querySelectorAll("button")].find(
    (b) => b.textContent === label,
  );
  if (!button) throw new Error(`no ${label} button in: ${host.innerHTML}`);
  act(() => button.dispatchEvent(new MouseEvent("click", { bubbles: true })));
};

beforeEach(() => {
  invoke.mockClear();
  window.matchMedia = vi.fn(() => ({
    matches: true,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
  delete document.documentElement.dataset.theme;
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("picking a theme", () => {
  it("paints before the write lands", () => {
    // Not a shortcut: this setting's whole result is the screen, and a round
    // trip to the disk before anything changes reads as the click not landing.
    const host = mount(<ThemePicker theme="system" />);
    click(host, "Light");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("still writes the preference through to settings", () => {
    const host = mount(<ThemePicker theme="system" />);
    click(host, "Dark");
    expect(invoke).toHaveBeenCalledWith("set_theme", { theme: "dark" });
  });

  it("marks the current choice for a screen reader, not just visually", () => {
    const host = mount(<ThemePicker theme="dark" />);
    const checked = [...host.querySelectorAll('[aria-checked="true"]')];
    expect(checked).toHaveLength(1);
    expect(checked[0].textContent).toBe("Dark");
  });

  it("says following the system is what following the system means", () => {
    // "System" is the only choice whose behavior is not self-evident from its
    // label — that it keeps tracking, rather than copying the value once.
    const host = mount(<ThemePicker theme="system" />);
    expect(host.textContent).toMatch(/follows your system/i);
  });
});
