import { describe, expect, it } from "vitest";

import type { BackgroundJob } from "../bindings/BackgroundJob";
import { loopbackServer, servedBy } from "./preview";

const job = (patch: Partial<BackgroundJob>): BackgroundJob => ({
  id: 1,
  command: "pnpm dev",
  running: true,
  stopped: false,
  ran_for: 3,
  status: "still running after 3s",
  ...patch,
});

describe("which server an address is", () => {
  it("is the same server by any loopback name", () => {
    const a = loopbackServer("http://localhost:5173/");
    expect(a).not.toBeNull();
    expect(loopbackServer("http://127.0.0.1:5173/app")).toBe(a);
    // Not a name the window may frame. Rust reports such a server as localhost.
    expect(loopbackServer("http://[::1]:5173")).toBeNull();
    expect(loopbackServer("http://localhost:5174/")).not.toBe(a);
    expect(loopbackServer("https://localhost:5173/")).not.toBe(a);
  });

  it("is nothing off this machine", () => {
    expect(loopbackServer("http://192.168.1.5:5173/")).toBeNull();
    expect(loopbackServer("http://localhost.example.com/")).toBeNull();
    expect(loopbackServer("not a url")).toBeNull();
  });

  it("is served only while the command serving it runs", () => {
    const serving = job({ url: "http://localhost:5173/" });
    expect(servedBy([serving], "http://127.0.0.1:5173/")).toBe(true);
    expect(servedBy([serving], "http://localhost:8080/")).toBe(false);
    expect(servedBy([{ ...serving, running: false, stopped: true }], "http://localhost:5173/")).toBe(false);
    expect(servedBy([job({})], "http://localhost:5173/")).toBe(false);
  });
});
