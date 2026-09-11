// @vitest-environment jsdom
//
// Whether the painted layer is repainted when nothing in the box changed.
//
// Walking the completion list re-renders the editor on every arrow and every
// hover, and none of that moves a character. Nothing on screen differs either
// way, so only counting the paints can see it.
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DataTable } from "../lib/api";

let painted = 0;
vi.mock("../lib/sql", async (original) => {
  const actual = await original<typeof import("../lib/sql")>();
  return {
    ...actual,
    ink: (text: string) => {
      painted += 1;
      return actual.ink(text);
    },
  };
});

const { SqlEditor } = await import("./SqlEditor");

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let cleanup: (() => void)[] = [];
afterEach(() => {
  cleanup.forEach((fn) => fn());
  cleanup = [];
  painted = 0;
});

const TABLES: DataTable[] = [
  {
    name: "users",
    path: "data/users.parquet",
    rows: 4_200,
    columns: [
      { name: "user_id", kind: "number", type_name: "Int64", nullable: false },
      { name: "country", kind: "text", type_name: "Utf8", nullable: true },
    ],
  },
];

function Harness() {
  const [sql, setSql] = useState("");
  return <SqlEditor value={sql} onChange={setSql} onRun={() => {}} tables={TABLES} />;
}

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  act(() => root.render(<Harness />));
  cleanup.push(() => {
    act(() => root.unmount());
    host.remove();
  });
  return host;
}

const box = (host: HTMLElement) =>
  host.querySelector(".sql-input") as HTMLTextAreaElement;

function type(host: HTMLElement, text: string) {
  const area = box(host);
  const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
  act(() => {
    set.call(area, text);
    area.setSelectionRange(text.length, text.length);
    area.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function press(host: HTMLElement, key: string) {
  act(() => {
    box(host).dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
    );
  });
}

describe("the painted layer", () => {
  it("is not repainted while the completion list is walked", () => {
    const host = mount();
    type(host, "SELECT user");
    expect(host.querySelector(".sql-menu")).not.toBeNull();
    const before = painted;

    press(host, "ArrowDown");
    press(host, "ArrowDown");
    press(host, "ArrowUp");

    expect(painted).toBe(before);
  });
});
