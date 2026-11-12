// @vitest-environment jsdom
//
// Settings › About › Command line. What matters is that the button is offered
// only where it can do something, that it says when it will ask for a
// password, and that it names a `taurus` a terminal would find first.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { CliStatus } from "../../lib/api";

const cliStatus = vi.fn<() => Promise<CliStatus>>();
const installCli = vi.fn<() => Promise<CliStatus>>();

vi.mock("../../lib/api", () => ({
  cliStatus: () => cliStatus(),
  installCli: () => installCli(),
}));

import { CliSection } from "./CliSection";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let cleanup: (() => void)[] = [];

beforeEach(() => {
  cliStatus.mockReset();
  installCli.mockReset();
});

afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
});

async function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => root.render(<CliSection />));
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return host;
}

const READY: CliStatus = {
  version: "0.4.1",
  target: "/usr/local/bin/taurus",
  installed: false,
  needs_admin: true,
};

describe("the command line section", () => {
  it("offers no button where there's nothing to install", async () => {
    cliStatus.mockResolvedValue({
      ...READY,
      target: undefined,
      cannot_install: "This is a development build.",
    });
    const host = await mount();
    expect(host.querySelector("button")).toBeNull();
    expect(host.textContent).toContain("This is a development build.");
  });

  it("says where it goes and that it asks for a password", async () => {
    cliStatus.mockResolvedValue(READY);
    const host = await mount();
    expect(host.querySelector("button")!.textContent).toBe("Install the taurus command");
    expect(host.textContent).toContain("/usr/local/bin/taurus");
    expect(host.textContent).toContain("administrator password");
  });

  it("installs, and then names a taurus that comes first", async () => {
    cliStatus.mockResolvedValue(READY);
    installCli.mockResolvedValue({
      ...READY,
      installed: true,
      shadowed_by: "/Users/me/.cargo/bin/taurus",
    });
    const host = await mount();
    await act(async () => host.querySelector("button")!.click());
    expect(installCli).toHaveBeenCalledOnce();
    expect(host.textContent).toContain("updates with the app");
    expect(host.textContent).toContain("/Users/me/.cargo/bin/taurus");
    expect(host.querySelector("button")!.textContent).toBe("Reinstall");
  });

  it("says why an install failed and leaves the button", async () => {
    cliStatus.mockResolvedValue(READY);
    installCli.mockRejectedValue("Cancelled. Nothing was installed.");
    const host = await mount();
    await act(async () => host.querySelector("button")!.click());
    expect(host.textContent).toContain("Cancelled. Nothing was installed.");
    expect(host.querySelector("button")!.disabled).toBe(false);
  });
});
