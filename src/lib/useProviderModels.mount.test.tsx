// @vitest-environment jsdom
//
// Two providers asked in turn are two round trips, and the slow one's list must
// not land under the fast one's name.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { useProviderModels } from "./useProviderModels";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let land: Record<string, (fail?: boolean) => void>;
let shown: unknown;
const Probe = ({ id }: { id: string }) => {
  shown = useProviderModels(id).models;
  return null;
};

beforeEach(() => {
  land = {};
  invoke.mockReset();
  invoke.mockImplementation((_: string, raw: unknown) => {
    const id = (raw as { providerId: string }).providerId;
    return new Promise((resolve, reject) => {
      land[id] = (fail) => (fail ? reject("down") : resolve([{ id: `${id}-model` }]));
    });
  });
});

async function switching() {
  const root = createRoot(document.createElement("div"));
  await act(async () => root.render(<Probe id="slow" />));
  await act(async () => root.render(<Probe id="fast" />));
  await act(async () => land.fast());
  expect(shown).toEqual([{ id: "fast-model" }]);
}

it("keeps the list of the provider in force when an older one answers late", async () => {
  await switching();
  await act(async () => land.slow());
  expect(shown).toEqual([{ id: "fast-model" }]);
});

it("keeps it when an older one fails late, too", async () => {
  await switching();
  await act(async () => land.slow(true));
  expect(shown).toEqual([{ id: "fast-model" }]);
});
