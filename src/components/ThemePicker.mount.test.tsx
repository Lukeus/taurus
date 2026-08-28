// @vitest-environment jsdom
//
// Mounted rather than rendered to a string, because the question here is what a
// click does — and what it does before the answer comes back from Rust.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/** Answers per command, so a test can seed the theme list. */
let themes: unknown[] = [];
const invoke = vi.fn((command: string, ..._args: unknown[]) =>
  Promise.resolve(command === "list_themes" ? themes : []),
);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [string])),
  Channel: class {},
}));

import { ThemePicker } from "./Settings";
import { useStore } from "../state/store";
import type { AppStatus, CustomTheme } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mount = (node: React.ReactNode) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  act(() => createRoot(host).render(node));
  return host;
};

/**
 * Mounts, and lets the theme list arrive before anything is asserted.
 *
 * Every mount of this screen asks Rust what is on disk, so every one of them
 * has a state update in flight when the render returns. Settling here rather
 * than in each test is what keeps the file free of `act` warnings that mean
 * nothing about the behaviour under test.
 */
const open = async (node: React.ReactNode) => {
  const host = mount(node);
  await act(async () => {});
  return host;
};

/**
 * Presses a button and lets what it started finish.
 *
 * Every control on this screen paints, then writes, then refreshes — three
 * steps across two awaits — so a press that was not settled leaves state
 * landing after the test has ended.
 */
const click = async (host: HTMLElement, label: string) => {
  const button = [...host.querySelectorAll("button")].find(
    (b) => b.textContent === label,
  );
  if (!button) throw new Error(`no ${label} button in: ${host.innerHTML}`);
  await act(async () => {
    button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
};

/** A custom theme, with only the fields this screen reads filled in. */
const theme = (over: Partial<CustomTheme> = {}): CustomTheme => ({
  id: "midnight",
  name: "Midnight",
  path: "/Users/x/.taurus/themes/midnight.json",
  scope: "global",
  dark: { accent: "#b48cff" },
  light: {},
  fonts: { display: null, body: null, mono: null },
  wordmark: null,
  logo: null,
  shape: { radius: null, gutter: null, "rail-gutter": null },
  modes: "both",
  ...over,
});

/** Puts a theme in force, the way a status from Rust would. */
const inForce = (custom: CustomTheme | null) =>
  useStore.setState({ status: { theme: custom } as unknown as AppStatus });

beforeEach(() => {
  themes = [];
  inForce(null);
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
  it("paints before the write lands", async () => {
    // trip to the disk before anything changes reads as the click not landing.
    const host = await open(<ThemePicker theme="system" />);
    await click(host, "Light");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("still writes the preference through to settings", async () => {
    const host = await open(<ThemePicker theme="system" />);
    await click(host, "Dark");
    expect(invoke).toHaveBeenCalledWith("set_theme", { theme: "dark" });
  });

  it("marks the current choice for a screen reader, not just visually", async () => {
    // — which palette, and whose colours — and each marks its own selection.
    const host = await open(<ThemePicker theme="dark" />);
    const checked = [
      ...host.querySelectorAll('[aria-label="Theme"] [aria-checked="true"]'),
    ];
    expect(checked).toHaveLength(1);
    expect(checked[0].textContent).toBe("Dark");
  });

  it("says following the system is what following the system means", async () => {
    // label — that it keeps tracking, rather than copying the value once.
    const host = await open(<ThemePicker theme="system" />);
    expect(host.textContent).toMatch(/follows your system/i);
  });
});

describe("picking a brand", () => {
  it("offers the app's own palette as a first-class choice, selected", async () => {
    // read where they are.
    const host = await open(<ThemePicker theme="dark" />);
    const checked = [
      ...host.querySelectorAll('[aria-label="Brand"] [aria-checked="true"]'),
    ];
    expect(checked).toHaveLength(1);
    expect(checked[0].textContent).toBe("Taurus");
  });

  it("lists what is on disk", async () => {
    themes = [theme(), theme({ id: "acme", name: "Acme" })];
    const host = await open(<ThemePicker theme="dark" />);
    const labels = [
      ...host.querySelectorAll('[aria-label="Brand"] button'),
    ].map((b) => b.textContent);
    expect(labels).toEqual(["Taurus", "Midnight", "Acme"]);
  });

  it("paints before the write lands, the same as the mode does", async () => {
    themes = [theme({ dark: { ink: "#010203" } })];
    const host = await open(<ThemePicker theme="dark" />);
    await click(host, "Midnight");
    expect(document.documentElement.style.getPropertyValue("--lk-ink")).toBe(
      "#010203",
    );
    expect(invoke).toHaveBeenCalledWith("set_theme_id", { id: "midnight" });
  });

  it("goes back to the built-in palette without leaving its tokens behind", async () => {
    // The bug this exists for: a version of `applyTheme` that only ever wrote
    // custom properties left the previous theme's ink standing after a switch
    // to one that does not name it.
    themes = [theme({ dark: { ink: "#010203" } })];
    const host = await open(<ThemePicker theme="dark" />);
    await click(host, "Midnight");
    await click(host, "Taurus");
    expect(document.documentElement.style.getPropertyValue("--lk-ink")).toBe("");
  });
});

describe("a theme that paints only one mode", () => {
  it("says so rather than leaving two controls that do nothing", async () => {
    inForce(theme({ modes: "dark_only", light: {} }));
    const host = await open(<ThemePicker theme="dark" />);
    const modes = [
      ...host.querySelectorAll<HTMLButtonElement>('[aria-label="Theme"] button'),
    ];
    expect(modes.every((b) => b.disabled)).toBe(true);
    expect(host.textContent).toContain("only a dark palette");
  });

  it("leaves the choice alone for a theme that paints both", async () => {
    inForce(theme({ modes: "both" }));
    const host = await open(<ThemePicker theme="dark" />);
    const modes = [
      ...host.querySelectorAll<HTMLButtonElement>('[aria-label="Theme"] button'),
    ];
    expect(modes.some((b) => b.disabled)).toBe(false);
  });
});
