import { describe, expect, it } from "vitest";

import { describe as say } from "./TracingTab";

describe("what this run is doing with its traces", () => {
  it("says it's exporting when that's what's saved", () => {
    expect(say({ endpoint: "http://localhost:4318", from_env: false }, "http://localhost:4318")).toBe(
      "Exporting to http://localhost:4318 now.",
    );
    expect(say({ from_env: false }, "")).toBe("Not exporting.");
  });

  it("says a saved change waits for a restart", () => {
    expect(say({ from_env: false }, "http://collector:4318")).toContain("Restart Taurus to start");
    expect(say({ endpoint: "http://old:4318", from_env: false }, "http://new:4318")).toContain(
      "Restart Taurus to use the saved one",
    );
    expect(say({ endpoint: "http://old:4318", from_env: false }, "")).toContain(
      "Restart Taurus to stop",
    );
  });

  it("names the variable when it wins over the field", () => {
    expect(say({ endpoint: "http://env:4318", from_env: true }, "http://saved:4318")).toContain(
      "OTEL_EXPORTER_OTLP_ENDPOINT",
    );
  });

  it("says why an endpoint exported nothing", () => {
    expect(
      say({ endpoint: "http://bad", from_env: false, failed: "invalid URI" }, "http://bad"),
    ).toBe("This run couldn't export to http://bad: invalid URI");
  });
});
