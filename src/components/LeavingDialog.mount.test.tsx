// @vitest-environment jsdom
//
// The offer made when a conversation is set to unattended. What matters is
// that nothing is granted without a press, and that unpicking a rule keeps it
// out of what the press grants.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { LeavingDialog, describeRule } from "./LeavingDialog";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

const RULES = ["run_command:cargo", "run_command:cd", "write_file"];

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const granted: string[][] = [];
  let closed = 0;
  act(() =>
    root.render(
      <LeavingDialog
        rules={RULES}
        onGrant={(rules) => granted.push(rules)}
        onClose={() => {
          closed += 1;
        }}
      />,
    ),
  );
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  const chips = () =>
    [...host.querySelectorAll<HTMLButtonElement>(".leaving-rules button")];
  const grant = () => host.querySelector<HTMLButtonElement>("button.primary")!;
  return { host, chips, grant, granted, closed: () => closed };
}

describe("the leaving dialog", () => {
  it("offers every rule, picked, and grants them all on one press", () => {
    const { chips, grant, granted } = mount();
    expect(chips().map((c) => c.textContent)).toEqual([
      "run cargo",
      "run cd",
      "write_file",
    ]);
    expect(chips().every((c) => c.getAttribute("aria-pressed") === "true")).toBe(true);
    expect(grant().textContent).toContain("all");

    act(() => grant().click());
    expect(granted).toEqual([RULES]);
  });

  it("keeps an unpicked rule out of the grant, and grants nothing when none is picked", () => {
    const { chips, grant, granted } = mount();
    act(() => chips()[1].click());
    expect(chips()[1].getAttribute("aria-pressed")).toBe("false");
    expect(grant().textContent).toContain("2");

    act(() => grant().click());
    expect(granted).toEqual([["run_command:cargo", "write_file"]]);

    act(() => chips()[0].click());
    act(() => chips()[2].click());
    expect(grant().disabled).toBe(true);
  });

  it("closes without granting", () => {
    const { host, granted, closed } = mount();
    const leave = [...host.querySelectorAll("button")].find(
      (b) => b.textContent === "Leave as is",
    )!;
    act(() => leave.click());
    expect(closed()).toBe(1);
    expect(granted).toEqual([]);
  });

  it("describes a rule the way its prompt did", () => {
    expect(describeRule("run_command:cargo")).toBe("run cargo");
    expect(describeRule("fetch_url:docs.rs")).toBe("fetch_url docs.rs");
    expect(describeRule("write_file")).toBe("write_file");
  });
});
