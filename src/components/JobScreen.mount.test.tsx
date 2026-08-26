// @vitest-environment jsdom
//
// Mounted rather than rendered to a string, because the two things worth
// checking here are both behaviour: which tab a click selects, and whether the
// pane follows a build down as it prints.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DockTabs, JobScreen } from "./JobScreen";
import type { BackgroundJob } from "../lib/api";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const job = (patch: Partial<BackgroundJob> = {}): BackgroundJob => ({
  id: 3,
  command: "cargo build --release",
  running: true,
  stopped: false,
  code: undefined,
  ran_for: 12,
  status: "still running after 12s",
  ...patch,
});

const into = (node: React.ReactNode) => {
  const host = document.createElement("div");
  document.body.appendChild(host);
  act(() => createRoot(host).render(node));
  return host;
};

afterEach(() => {
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("the tab strip", () => {
  it("puts the shell first and every command after it", () => {
    // The shell is the tab that is always there, and a strip that reordered
    // itself as builds came and went would move the one thing under the cursor
    // that never changes.
    const host = into(
      <DockTabs
        jobs={[job({ id: 3 }), job({ id: 4, command: "npm run dev" })]}
        watching={null}
        onWatch={() => {}}
      />,
    );
    const tabs = [...host.querySelectorAll(".dock-tab")].map((t) => t.textContent);
    expect(tabs[0]).toBe("Terminal");
    expect(tabs[1]).toContain("#3");
    expect(tabs[2]).toContain("npm run dev");
  });

  it("says which tab is showing, to a screen reader as well as a colour", () => {
    const host = into(
      <DockTabs jobs={[job()]} watching={3} onWatch={() => {}} />,
    );
    const on = host.querySelector('[aria-selected="true"]');
    expect(on?.textContent).toContain("#3");
    expect(on?.className).toContain("on");
  });

  it("carries the whole command, however short the tab is", () => {
    // The label is clipped to fit a strip. What was actually run is the thing
    // worth being able to check, so it is on the element rather than lost.
    const host = into(
      <DockTabs
        jobs={[job({ command: "cargo test --workspace --all-features" })]}
        watching={null}
        onWatch={() => {}}
      />,
    );
    const tab = host.querySelectorAll(".dock-tab")[1];
    expect(tab.getAttribute("title")).toBe("cargo test --workspace --all-features");
    expect(tab.textContent).toContain("…");
  });

  it("switches when a tab is pressed", () => {
    const watch = vi.fn();
    const host = into(<DockTabs jobs={[job()]} watching={null} onWatch={watch} />);
    act(() => (host.querySelectorAll(".dock-tab")[1] as HTMLButtonElement).click());
    expect(watch).toHaveBeenCalledWith(3);
    act(() => (host.querySelectorAll(".dock-tab")[0] as HTMLButtonElement).click());
    expect(watch).toHaveBeenLastCalledWith(null);
  });

  it("marks a failed command with a character and not only a colour", () => {
    const host = into(
      <DockTabs
        jobs={[job({ running: false, code: 1 })]}
        watching={null}
        onWatch={() => {}}
      />,
    );
    const tab = host.querySelectorAll(".dock-tab")[1];
    expect(tab.className).toContain("failed");
    expect(tab.querySelector(".dock-tab-mark")?.textContent).toBe("✗");
  });
});

describe("a command's pane", () => {
  it("shows what it printed", () => {
    const host = into(
      <JobScreen job={job()} text="Compiling taurus\n" problem={null} onStop={() => {}} />,
    );
    expect(host.querySelector(".job-out")?.textContent).toContain("Compiling taurus");
  });

  it("says how it is doing in the host's own words", () => {
    // Said once, in `say`, so the window and `check_command` cannot describe
    // the same command two ways.
    const host = into(
      <JobScreen job={job()} text="" problem={null} onStop={() => {}} />,
    );
    expect(host.querySelector(".job-status")?.textContent).toBe(
      "still running after 12s",
    );
  });

  it("offers a stop while it is running, and not after", () => {
    const stop = vi.fn();
    const host = into(<JobScreen job={job()} text="" problem={null} onStop={stop} />);
    const button = host.querySelector(".pill") as HTMLButtonElement;
    act(() => button.click());
    expect(stop).toHaveBeenCalledOnce();

    const done = into(
      <JobScreen
        job={job({ running: false, code: 0 })}
        text=""
        problem={null}
        onStop={() => {}}
      />,
    );
    expect(done.querySelector(".pill")).toBeNull();
  });

  it("tells a silent command apart from one that has not started printing", () => {
    // An empty frame reads as a pane that failed to draw. The two cases are
    // different facts and the second one is final.
    const running = into(
      <JobScreen job={job()} text="" problem={null} onStop={() => {}} />,
    );
    expect(running.querySelector(".job-out")?.textContent).toContain(
      "Nothing printed yet",
    );
    const done = into(
      <JobScreen
        job={job({ running: false, code: 0 })}
        text=""
        problem={null}
        onStop={() => {}}
      />,
    );
    expect(done.querySelector(".job-out")?.textContent).toContain("printed nothing");
  });

  it("says when a stop did not take", () => {
    const host = into(
      <JobScreen
        job={job()}
        text=""
        problem="that process is already gone"
        onStop={() => {}}
      />,
    );
    expect(host.querySelector(".dock-problem")?.textContent).toContain("already gone");
  });

  it("names the command it is the output of", () => {
    // The pane is a scroll region a screen reader can land in, and "output"
    // with no owner is the least useful thing it could be called.
    const host = into(
      <JobScreen job={job()} text="hi" problem={null} onStop={() => {}} />,
    );
    expect(host.querySelector(".job-out")?.getAttribute("aria-label")).toBe(
      "Output of cargo build --release",
    );
  });
});
