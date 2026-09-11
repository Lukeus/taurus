import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { ChartCard } from "./ChartCard";

describe("a chart", () => {
  it("names each bar and its value, which otherwise shows only on hover", () => {
    // A screen reader got a row of unlabelled boxes of different heights.
    const html = renderToStaticMarkup(
      <ChartCard
        view={{
          type: "chart",
          title: "Tool calls",
          caption: null,
          labels: ["read_file", "grep"],
          series: [{ name: "calls", unit: "", values: [12, 3.5] }],
        }}
      />,
    );
    expect(html).toContain('aria-label="read_file: 12"');
    expect(html).toContain('aria-label="grep: 3.5"');
  });

  it("carries the series' unit with the value", () => {
    const html = renderToStaticMarkup(
      <ChartCard
        view={{
          type: "chart",
          title: "Slowest step",
          caption: null,
          labels: ["build"],
          series: [{ name: "time", unit: "s", values: [42] }],
        }}
      />,
    );
    expect(html).toContain('aria-label="build: 42s"');
  });
});
