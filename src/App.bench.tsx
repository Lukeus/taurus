// @vitest-environment jsdom
//
// What a streamed token costs the whole app, rather than the transcript alone.
//
// `Transcript.bench.tsx` measures the component. This measures the wiring
// around it, and the thing it is really asking is whether anything *else*
// redraws when a token lands: the rail with its list of conversations, the
// topbar, the model picker. None of them have anything new to say, and none of
// them should be re-rendered to establish that.
//
// The lever is `App`'s store subscription. Subscribing to the whole store —
// which is the default, and what this used to do — means every frame of every
// turn re-renders all of it. With `App` reading only the fields it draws, and
// the entries read where they are drawn, the number below roughly halves and
// its floor drops to the transcript's own cost:
//
//   whole store   1.70ms mean, 1.12ms min
//   by field      0.84ms mean, 0.15ms min
//
// A number that climbs back towards the first pair means something put the
// transcript back into `App`'s subscription.
//
//   pnpm bench
import { act } from "react";
import { createRoot } from "react-dom/client";
import { bench, describe, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) =>
    cmd === "get_status"
      ? { providers: [{ id: "ollama", default_model: "m", models: [] }], settings: { theme: "dark", last_provider: "ollama" }, workspace: "/w", branch: "main", skill_count: 3, agent_count: 2, mcp_servers: [] }
      : [],
  ),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const { default: App } = await import("./App");
const { useStore } = await import("./state/store");

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
Element.prototype.scrollIntoView = () => {};

/** A conversation of `turns` exchanges, in the ordinary shape. */
function history(turns: number) {
  const out: unknown[] = [];
  for (let t = 0; t < turns; t++) {
    out.push({ kind: "user", id: `u${t}`, text: `question ${t}`, images: [] });
    out.push({ kind: "assistant", id: `a${t}`, open: false, thinking: "", text: `## Answer ${t}\n\nprose\n` });
    for (let k = 0; k < 4; k++)
      out.push({ kind: "tool", id: `t${t}-${k}`, name: "read_file", preview: "x", status: "ok", steps: [], output: "ok", startedAt: 0, endedAt: 5 });
  }
  return out;
}

/** A rail with `n` conversations in it, which is what redraws for free. */
function sessions(n: number) {
  return Array.from({ length: n }, (_, i) => ({
    id: `s${i}`, title: `Conversation ${i}`, updated_at: 1700000000 + i,
    workspace: "/w", provider_id: "ollama", model: "m", branch: "main",
  }));
}

function mount(turns: number, convos: number) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const base = history(turns);
  act(() => { root.render(<App />); });
  // After the mount, so `init`'s own writes do not land on top of it.
  act(() => { useStore.setState({ sessions: sessions(convos), busy: true, entries: base } as never); });
  let text = "";
  return {
    frame() {
      text += "token ";
      useStore.setState({ entries: [...base, { kind: "assistant", id: "live", open: true, thinking: "", text }] } as never);
    },
    stop() { act(() => root.unmount()); host.remove(); },
  };
}

describe("one streamed token, whole app", () => {
  for (const [turns, convos] of [[20, 50], [50, 50]] as const) {
    let live: ReturnType<typeof mount>;
    bench(`${turns} turns, ${convos} conversations in the rail`, () => act(() => live.frame()), {
      setup: () => { live = mount(turns, convos); },
      teardown: () => live.stop(),
    });
  }
});
