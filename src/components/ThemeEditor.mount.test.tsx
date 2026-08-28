// @vitest-environment jsdom
//
// Mounted rather than rendered to a string, because everything worth asserting
// about this screen is a side effect on the document: it previews on the real
// window, and putting that back is the part most easily got wrong.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(() => Promise.resolve(""));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...(args as [])),
  Channel: class {},
}));

import { ThemeEditor } from "./ThemeEditor";
import type { CustomTheme } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

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

const mount = (node: React.ReactNode) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  act(() => createRoot(host).render(node));
  return host;
};

const painted = (token: string) =>
  document.documentElement.style.getPropertyValue(token);

const type = (host: HTMLElement, label: string, value: string) => {
  const input = host.querySelector<HTMLInputElement>(`[aria-label="${label}"]`);
  if (!input) throw new Error(`no field labelled ${label}`);
  act(() => {
    // React tracks the last value it wrote, so setting `.value` directly and
    // firing `input` is ignored as a no-op change. The native setter is what
    // gets past that, and it is the standard way to drive a controlled input.
    Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
};

const click = (host: HTMLElement, label: string) => {
  const button = [...host.querySelectorAll("button")].find(
    (b) => b.textContent === label || b.getAttribute("aria-label") === label,
  );
  if (!button) throw new Error(`no ${label} button`);
  act(() => button.dispatchEvent(new MouseEvent("click", { bubbles: true })));
};

beforeEach(() => {
  invoke.mockClear();
  document.documentElement.removeAttribute("style");
  delete document.documentElement.dataset.theme;
});

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("previewing a draft", () => {
  it("paints the theme being edited on the real window", () => {
    // Not on a swatch. A palette is how its colours look under each other at
    // the sizes this app uses them, and no tile the size of a postcard can say
    // whether an accent survives a 10px label in the rail.
    mount(
      <ThemeEditor editing={theme({ dark: { ink: "#010203" } })} mode="dark" onClose={() => {}} onSaved={() => {}} />,
    );
    expect(painted("--lk-ink")).toBe("#010203");
  });

  it("repaints as a colour is typed", () => {
    const host = mount(
      <ThemeEditor editing={theme()} mode="dark" onClose={() => {}} onSaved={() => {}} />,
    );
    type(host, "ink hex", "#0a0b0c");
    expect(painted("--lk-ink")).toBe("#0a0b0c");
  });

  it("puts the window back on the way out, whichever way out was taken", () => {
    // Restoring inside each handler instead left Escape — and a click on the
    // scrim — painting somebody's abandoned draft until the next status
    // landed. Unmounting is the one path all four ways off this screen go
    // through, so the restore hangs off that.
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    const saved = theme({ dark: { ink: "#010203" } });
    act(() =>
      root.render(
        <ThemeEditor editing={saved} mode="dark" onClose={() => {}} onSaved={() => {}} />,
      ),
    );

    type(host, "ink hex", "#ff0000");
    expect(painted("--lk-ink")).toBe("#ff0000");

    act(() => root.unmount());
    expect(painted("--lk-ink")).toBe("#010203");
  });

  it("leaves nothing behind when the editor was opened on no theme at all", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    act(() =>
      root.render(
        <ThemeEditor editing={null} mode="dark" onClose={() => {}} onSaved={() => {}} />,
      ),
    );
    type(host, "ink hex", "#ff0000");
    expect(painted("--lk-ink")).toBe("#ff0000");

    act(() => root.unmount());
    expect(painted("--lk-ink")).toBe("");
  });

  it("switches which palette is being edited, and shows it", () => {
    const host = mount(
      <ThemeEditor
        editing={theme({ dark: { ink: "#010203" }, light: { ink: "#fefefe" } })}
        mode="dark"
        onClose={() => {}}
        onSaved={() => {}}
      />,
    );
    expect(painted("--lk-ink")).toBe("#010203");
    click(host, "Light palette");
    expect(painted("--lk-ink")).toBe("#fefefe");
    expect(document.documentElement.dataset.theme).toBe("light");
  });
});

describe("stating a colour, or inheriting it", () => {
  it("counts what this palette actually sets", () => {
    // The answer to "have I done the light one yet", which is the thing this
    // editor is easiest to get wrong on.
    const host = mount(
      <ThemeEditor
        editing={theme({ dark: { ink: "#010203", accent: "#b48cff" } })}
        mode="dark"
        onClose={() => {}}
        onSaved={() => {}}
      />,
    );
    expect(host.textContent).toContain("2 of 14 set");
  });

  it("gives a colour back to the stylesheet when it is cleared", () => {
    const host = mount(
      <ThemeEditor editing={theme({ dark: { ink: "#010203" } })} mode="dark" onClose={() => {}} onSaved={() => {}} />,
    );
    click(host, "Use the built-in ink");
    expect(painted("--lk-ink")).toBe("");
    expect(host.textContent).toContain("0 of 14 set");
  });

  it("has nothing to clear on a colour it inherits", () => {
    const host = mount(
      <ThemeEditor editing={theme({ dark: {} })} mode="dark" onClose={() => {}} onSaved={() => {}} />,
    );
    const clear = host.querySelector<HTMLButtonElement>('[aria-label="Use the built-in ink"]');
    expect(clear?.disabled).toBe(true);
  });
});

describe("saving", () => {
  it("writes only the keys that were set", async () => {
    // The file is a thing people read and diff. A theme that changes one
    // colour should be four lines, not a transcription of the palette.
    const host = mount(
      <ThemeEditor
        editing={theme({ dark: { accent: "#b48cff" } })}
        mode="dark"
        onClose={() => {}}
        onSaved={() => {}}
      />,
    );
    click(host, "Save theme");
    await act(async () => {});

    const [command, payload] = invoke.mock.calls[0] as unknown as [
      string,
      { id: string; scope: string; theme: Record<string, unknown> },
    ];
    expect(command).toBe("save_theme");
    expect(payload.id).toBe("midnight");
    expect(payload.theme.dark).toEqual({ accent: "#b48cff" });
    expect(payload.theme.light).toEqual({});
    expect(payload.theme.shape).toEqual({
      radius: null,
      gutter: null,
      "rail-gutter": null,
    });
  });

  it("saves a workspace theme back to the workspace", async () => {
    // A theme a repository ships stays in the repository when it is edited,
    // rather than being quietly forked into the user's home directory where
    // the project can never see it again.
    const host = mount(
      <ThemeEditor
        editing={theme({ scope: "workspace" })}
        mode="dark"
        onClose={() => {}}
        onSaved={() => {}}
      />,
    );
    click(host, "Save theme");
    // The save is three awaits deep; without settling it, the state it sets on
    // the way back lands after the test has ended.
    await act(async () => {});
    expect(invoke).toHaveBeenCalledWith(
      "save_theme",
      expect.objectContaining({ scope: "workspace" }),
    );
  });

  it("will not save a theme with no name to store it under", () => {
    const host = mount(
      <ThemeEditor editing={null} mode="dark" onClose={() => {}} onSaved={() => {}} />,
    );
    const save = [...host.querySelectorAll("button")].find(
      (b) => b.textContent === "Save theme",
    );
    expect(save?.disabled).toBe(true);
    expect(host.textContent).toContain("needs a name");
  });
});

describe("contrast", () => {
  it("warns about a pair that will not be readable, and says where it is", () => {
    // A warning that cannot be tied to something on screen is a warning people
    // turn off, so each one names the thing it is about rather than the two
    // tokens it is between.
    const host = mount(
      <ThemeEditor
        editing={theme({
          dark: {
            ink: "#101820",
            "surface-1": "#101820",
            "surface-2": "#101820",
            text: "#141c24",
            "text-dim": "#141c24",
            "text-faint": "#141c24",
            accent: "#141c24",
            "accent-hover": "#141c24",
            "on-accent": "#141c24",
            ok: "#141c24",
            warn: "#141c24",
            danger: "#141c24",
            line: "#101820",
          },
        })}
        mode="dark"
        onClose={() => {}}
        onSaved={() => {}}
      />,
    );
    expect(host.textContent).toMatch(/hard to read/);
    expect(host.textContent).toContain("A sentence in the transcript");
  });

  it("says nothing when every pair clears its floor", () => {
    const host = mount(
      <ThemeEditor
        editing={theme({
          dark: {
            ink: "#000000",
            "surface-1": "#000000",
            "surface-2": "#000000",
            line: "#666666",
            text: "#ffffff",
            "text-dim": "#ffffff",
            "text-faint": "#ffffff",
            accent: "#ffffff",
            "accent-hover": "#ffffff",
            "on-accent": "#000000",
            ok: "#ffffff",
            warn: "#ffffff",
            danger: "#ffffff",
          },
        })}
        mode="dark"
        onClose={() => {}}
        onSaved={() => {}}
      />,
    );
    expect(host.textContent).not.toMatch(/hard to read/);
  });
});
