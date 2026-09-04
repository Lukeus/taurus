// @vitest-environment jsdom
//
// Mounted rather than rendered to a string, because what matters about a delete
// is the step between pressing it and it happening.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Rail, THEME_LABEL } from "./Rail";
import type { SessionMeta, Theme } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const now = Math.floor(Date.now() / 1000);

const session = (id: string, title: string): SessionMeta => ({
  id,
  workspace: "/Users/x/code/taurus",
  model: "qwen3.6:27b",
  started: now,
  updated: now,
  title,
});

const mount = (props: Partial<Parameters<typeof Rail>[0]> = {}) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  act(() =>
    createRoot(host).render(
      <Rail
        width={236}
        workspace="/Users/x/code/taurus-ai-shell"
        sessions={[session("a", "Rename it"), session("b", "Summarize it")]}
        currentId={undefined}
        changedCount={0}
        branch={null}
        busy={false}
        skillCount={12}
        agentCount={3}
        noteCount={2}
        mcp={{ total: 2, connected: 2 }}
      jobsRunning={0}
        health={{ state: "connected", id: "ollama", models: 4 }}
        theme="dark"
        brand={null}
        onPickWorkspace={() => {}}
        onNew={() => {}}
        onOpen={() => {}}
        onDelete={() => {}}
        onTheme={() => {}}
        onSkills={() => {}}
        onAgents={() => {}}
        onMemory={() => {}}
        onUsage={() => {}}
        onTraces={() => {}}
        onMcp={() => {}}
        onTerminal={() => {}}
        onSettings={() => {}}
        {...props}
      />,
    ),
  );
  return host;
};

const press = (host: HTMLElement, selector: string, index = 0) => {
  const button = host.querySelectorAll<HTMLButtonElement>(selector)[index];
  if (!button) throw new Error(`no ${selector}[${index}] in: ${host.innerHTML}`);
  act(() => button.dispatchEvent(new MouseEvent("click", { bubbles: true })));
};

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("a fold that is shut", () => {
  /** Collapses `name` the way a previous session would have left it. */
  const shut = (name: string) =>
    localStorage.setItem("taurus.railSections", JSON.stringify([name]));

  afterEach(() => localStorage.clear());

  it("still says a command is running behind it", () => {
    // Folding a section is a way to stop looking at it, not a way to stop
    // being told something in it is still going. A build the model started
    // while this was shut is the job nothing else on screen would mention.
    shut("connections");
    const host = mount({ jobsRunning: 2 });
    const header = host.querySelector(".rail-group[data-shut]")!;
    expect(header.textContent).toContain("Connections");
    expect(header.querySelector("[data-live]")?.textContent).toBe("2");
  });

  it("still marks a server that is not answering", () => {
    shut("connections");
    const host = mount({ mcp: { total: 3, connected: 1 } });
    const header = host.querySelector(".rail-group[data-shut]")!;
    expect(header.querySelector(".dot.warn")).not.toBeNull();
    // And says which, because a mark that cannot say what is wrong is a mark
    // people learn to ignore.
    expect(header.getAttribute("data-tip")).toContain("not connected");
  });

  it("draws nothing at all when there is nothing happening", () => {
    shut("connections");
    const host = mount({ jobsRunning: 0, mcp: { total: 2, connected: 2 } });
    const header = host.querySelector(".rail-group[data-shut]")!;
    expect(header.querySelector("[data-live]")).toBeNull();
    expect(header.querySelector(".dot.warn")).toBeNull();
  });

  it("does not leave the rows it hid reachable by keyboard", () => {
    // `display: none` still answers a screen reader and still takes Tab in
    // some engines, so the children are not rendered rather than hidden.
    shut("agent");
    const host = mount();
    expect(host.textContent).not.toContain("Skills");
  });
});

describe("deleting a conversation", () => {
  it("asks before it does anything", () => {
    const onDelete = vi.fn();
    const host = mount({ onDelete });
    press(host, '[aria-label="Delete Rename it"]');
    expect(onDelete).not.toHaveBeenCalled();
    expect(host.textContent).toContain("delete this and its undo history?");
  });

  it("deletes the conversation that was armed, on the second press", () => {
    const onDelete = vi.fn();
    const host = mount({ onDelete });
    press(host, '[aria-label="Delete Summarize it"]');
    press(host, ".rail-delete[data-confirm]");
    expect(onDelete).toHaveBeenCalledWith("b");
  });

  it("can be backed out of", () => {
    const onDelete = vi.fn();
    const host = mount({ onDelete });
    press(host, '[aria-label="Delete Rename it"]');
    press(host, '[aria-label="Keep this conversation"]');
    expect(onDelete).not.toHaveBeenCalled();
    expect(host.textContent).not.toContain("delete this");
  });

  it("only ever has one question open", () => {
    // Two armed rows is two Delete buttons a few pixels apart, both of which
    // erase something different and neither of which says which.
    const host = mount();
    press(host, '[aria-label="Delete Rename it"]');
    press(host, '[aria-label="Delete Summarize it"]');
    expect(host.querySelectorAll(".rail-delete[data-confirm]")).toHaveLength(1);
  });

  it("leaves the other rows deletable while a turn is running", () => {
    // The backend refuses only the conversation the turn is in. Disabling the
    // rest would be the UI inventing a rule the app does not have.
    const host = mount({ currentId: "a", busy: true });
    const enabled = (label: string) =>
      !host.querySelector<HTMLButtonElement>(`[aria-label="Delete ${label}"]`)!
        .disabled;
    expect(enabled("Rename it")).toBe(false);
    expect(enabled("Summarize it")).toBe(true);
  });

  it("does not open the conversation it is asking about", () => {
    // The trash can sits inside the row that switches conversations, so a
    // click that bubbled would delete-prompt and navigate at the same time.
    const onOpen = vi.fn();
    const host = mount({ onOpen });
    press(host, '[aria-label="Delete Rename it"]');
    expect(onOpen).not.toHaveBeenCalled();
  });
});

describe("the theme row", () => {
  const nextFrom = (theme: Theme) => {
    const onTheme = vi.fn();
    const host = mount({ theme, onTheme });
    // Found by the word on it rather than by its index in the foot: this was
    // `.rail-link` number 2, and adding one link above it moved the assertion
    // onto a different button without failing until it did.
    const buttons = [
      ...host.querySelectorAll<HTMLButtonElement>(".rail-foot .rail-link"),
    ];
    const button = buttons.find((b) =>
      b.textContent?.includes(THEME_LABEL[theme]),
    );
    if (!button) throw new Error(`no theme button in: ${host.innerHTML}`);
    act(() => button.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    return onTheme.mock.calls[0]?.[0];
  };

  it("cycles all three preferences rather than toggling two", () => {
    // A light/dark switch here would quietly discard "follow the system",
    // which is both the default and the only one that can change on its own.
    expect(nextFrom("system")).toBe("light");
    expect(nextFrom("light")).toBe("dark");
    expect(nextFrom("dark")).toBe("system");
  });
});
