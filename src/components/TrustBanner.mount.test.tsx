// @vitest-environment jsdom
//
// What this banner *says* is the whole feature. Someone is being asked to let a
// folder they may have cloned a minute ago configure their agent, and the only
// thing that makes that a real decision rather than a reflex is that the page
// names what is waiting — particularly the command lines, which are the part
// that starts a process. So these tests are mostly about text.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PendingConfig, TrustStatus } from "../lib/api";
import { TrustBanner } from "./TrustBanner";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const EMPTY: PendingConfig = {
  skills: 0,
  agents: 0,
  mcp_servers: 0,
  mcp_commands: [],
  instructions: 0,
  permission_rules: 0,
  providers: false,
  search: false,
  settings: false,
};

const status = (pending: Partial<PendingConfig>, trusted = false): TrustStatus => {
  const filled = { ...EMPTY, ...pending };
  return {
    workspace: "/tmp/project",
    trusted,
    pending: filled,
    decision_needed: !trusted,
  };
};

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

const noop = () => {};

beforeEach(() => {
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("TrustBanner", () => {
  it("names the command an MCP server would run, not just the count", () => {
    const host = mount(
      <TrustBanner
        trust={status({ mcp_servers: 1, mcp_commands: ["probe: npx -y thing"] })}
        onTrust={noop}
        onDismiss={noop}
      />,
    );

    // The count alone is unanswerable. "npx -y thing" is a thing a person can
    // look at and decide about, and it is also the thing that would run.
    expect(host.textContent).toContain("npx -y thing");
  });

  it("says a committed allowlist would skip the permission prompt", () => {
    const host = mount(
      <TrustBanner
        trust={status({ permission_rules: 2 })}
        onTrust={noop}
        onDismiss={noop}
      />,
    );

    expect(host.textContent).toContain("2 standing permission grants");
    // Without this clause the row reads like a setting. It is the one entry
    // that hands over capability with no further prompt at all.
    expect(host.textContent).toContain("without asking");
  });

  it("says the user's own config is unaffected", () => {
    // The fear this heads off is "has Taurus stopped working?". It has not —
    // only this folder's layer is held back — and the banner has to say so, or
    // the fastest way out of the confusion is to click the wrong button.
    const host = mount(
      <TrustBanner trust={status({ skills: 3 })} onTrust={noop} onDismiss={noop} />,
    );
    expect(host.textContent).toContain("unaffected");
  });

  it("shows nothing when there is nothing to decide", () => {
    const host = mount(
      <TrustBanner
        trust={status({ skills: 3 }, true)}
        onTrust={noop}
        onDismiss={noop}
      />,
    );
    expect(host.textContent).toBe("");
  });

  it("keeps the two answers apart", () => {
    const trusted = vi.fn();
    const dismissed = vi.fn();
    const host = mount(
      <TrustBanner
        trust={status({ skills: 1 })}
        onTrust={trusted}
        onDismiss={dismissed}
      />,
    );

    click(host, "Not now");
    expect(dismissed).toHaveBeenCalledTimes(1);
    // Waving the banner off must not be a decision: nothing is recorded, and
    // in particular nothing grants.
    expect(trusted).not.toHaveBeenCalled();

    click(host, "Read this project's config");
    expect(trusted).toHaveBeenCalledTimes(1);
  });

  it("pluralizes so a single item does not read as a bug", () => {
    const host = mount(
      <TrustBanner trust={status({ skills: 1 })} onTrust={noop} onDismiss={noop} />,
    );
    expect(host.textContent).toContain("1 skill");
    expect(host.textContent).not.toContain("1 skills");
  });
});
