// Old transcripts can hold the same tool id more than once: the prompted fallback
// used to count from `ptc_1` on every request. A server can reuse one live, too.
// Each call still gets its own card and its own result.
import { describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import type { Message, SessionMeta } from "../lib/api";
import { answering, entriesFromMessages, foldEvents, foldTurn, linkDelegates, reduce, runFromMessages, useStore, wireId, type Entry } from "./store";
import { mergeAgents, NO_AGENTS } from "../lib/agents";

type Tool = Extract<Entry, { kind: "tool" }>;
const tools = (entries: Entry[]) => entries.filter((e): e is Tool => e.kind === "tool");

const said = (role: "user" | "assistant", text: string) =>
  ({ role, content: [{ type: "text", text }] }) as unknown as Message;
const said2 = (text: string) => said("assistant", text);
const calls = (...uses: [string, string][]) =>
  ({
    role: "assistant",
    content: uses.map(([id, path]) => ({ type: "tool_use", id, name: "read_file", input: { path } })),
  }) as unknown as Message;
const results = (...answers: [string, string][]) =>
  ({
    role: "user",
    content: answers.map(([id, output]) => ({
      type: "tool_result",
      tool_use_id: id,
      content: [{ type: "text", text: output }],
      is_error: false,
    })),
  }) as unknown as Message;

describe("a transcript with a repeated id", () => {
  it("answers each call with its own result, under ids that don't collide", () => {
    const [a, b, c] = tools(
      entriesFromMessages([
        said("user", "go"),
        calls(["ptc_1", "a"]),
        results(["ptc_1", "of a"]),
        calls(["ptc_1", "b"]),
        results(["ptc_1", "of b"]),
        calls(["ptc_1", "c"]),
        results(["ptc_1", "of c"]),
      ]),
    );
    expect([a.output, b.output, c.output]).toEqual(["of a", "of b", "of c"]);
    expect(a.preview).toContain('"a"');
    expect(c.preview).toContain('"c"');
    expect(new Set([a.id, b.id, c.id]).size).toBe(3);
    expect([wireId(a), wireId(b), wireId(c)]).toEqual(["ptc_1", "ptc_1", "ptc_1"]);
  });

  it("skips an old call left unanswered, for the nearer one", () => {
    const [old, next] = tools(
      entriesFromMessages([calls(["ptc_1", "a"]), calls(["ptc_1", "b"]), results(["ptc_1", "of b"])]),
    );
    expect(old.status).toBe("running");
    expect(next.output).toBe("of b");
  });

  it("answers repeats within one message in order, not swapped", () => {
    const [a, b] = tools(
      entriesFromMessages([
        calls(["ptc_1", "a"], ["ptc_1", "b"]),
        results(["ptc_1", "of a"], ["ptc_1", "of b"]),
      ]),
    );
    expect([a.output, b.output]).toEqual(["of a", "of b"]);
  });

  it("doesn't let a stray result overwrite a call already answered", () => {
    const [a] = tools(
      entriesFromMessages([calls(["ptc_1", "a"]), results(["ptc_1", "of a"]), results(["ptc_1", "stray"])]),
    );
    expect(a.output).toBe("of a");
  });

  it("doesn't take a real id that looks like a renamed one", () => {
    const cards = tools(
      entriesFromMessages([calls(["ptc_1", "a"], ["ptc_1_2", "b"], ["ptc_1", "c"])]),
    );
    expect(new Set(cards.map((c) => c.id)).size).toBe(3);
    expect(cards.map(wireId)).toEqual(["ptc_1", "ptc_1_2", "ptc_1"]);
  });

  it("gives a delegate's transcript only to the delegation under that id", () => {
    const spawn = {
      role: "assistant",
      content: [{ type: "tool_use", id: "ptc_1", name: "spawn_subagent", input: {} }],
    } as unknown as Message;
    const delegate = { id: "child", call: "ptc_1", agent: "x", started: 1 } as SessionMeta;
    const [read, spawned] = tools(
      linkDelegates(
        entriesFromMessages([calls(["ptc_1", "a"]), results(["ptc_1", "of a"]), spawn]),
        [delegate],
      ),
    );
    expect(read.transcript).toBeUndefined();
    expect(spawned.transcript?.session).toBe("child");

    // And with the read as the later of the two.
    const [first, later] = tools(
      linkDelegates(entriesFromMessages([spawn, results(["ptc_1", "x"]), calls(["ptc_1", "a"])]), [delegate]),
    );
    expect(first.transcript?.session).toBe("child");
    expect(later.transcript).toBeUndefined();
  });

  it("settles one of a delegate's calls per result", () => {
    const run = runFromMessages("call", { id: "s", model: "m", started: 0, updated: 0 } as SessionMeta, [
      calls(["ptc_1", "a"]),
      results(["ptc_1", "of a"]),
      calls(["ptc_1", "b"]),
    ]);
    expect(run.tools.map((t) => t.ok)).toEqual([true, undefined]);

    // A stray result doesn't change a call already settled.
    const failed = { ...results(["ptc_1", "stray"]) };
    (failed.content[0] as { is_error: boolean }).is_error = true;
    const stray = runFromMessages("call", { id: "s", model: "m", started: 0, updated: 0 } as SessionMeta, [
      calls(["ptc_1", "a"]),
      results(["ptc_1", "of a"]),
      failed,
    ]);
    expect(stray.tools[0].ok).toBe(true);
  });
});

describe("live events under a repeated id", () => {
  const started = (id: string, preview: string) =>
    ({ type: "tool_call_started", id, name: "read_file", preview }) as never;
  const finished = (id: string, output: string) =>
    ({ type: "tool_call_finished", id, ok: true, output }) as never;
  const progress = (id: string, label: string) => ({ type: "tool_progress", id, label }) as never;

  it("draws a new card, and leaves the finished one under that id alone", () => {
    const [old, next] = tools(
      foldEvents(
        [started("ptc_1", "a"), finished("ptc_1", "of a"), started("ptc_1", "b"), progress("ptc_1", "working"), finished("ptc_1", "of b")],
        [],
      ),
    );
    expect(old.output).toBe("of a");
    expect(old.steps).toEqual([]);
    expect(next.output).toBe("of b");
    expect(next.steps).toEqual(["working"]);
    expect(next.id).not.toBe(old.id);
    expect(wireId(next)).toBe("ptc_1");
  });

  it("leaves a card from the loaded transcript alone too", () => {
    const loaded = entriesFromMessages([calls(["ptc_1", "a"]), results(["ptc_1", "of a"])]);
    const [old, next] = tools(foldEvents([started("ptc_1", "b"), finished("ptc_1", "of b")], loaded));
    expect(old.output).toBe("of a");
    expect(next.output).toBe("of b");
    expect(next.id).not.toBe(old.id);
  });

  it("treats a running card read from disk as the call being caught up on", () => {
    const loaded = entriesFromMessages([calls(["ptc_1", "a"])]);
    const out = tools(foldEvents([started("ptc_1", "live"), finished("ptc_1", "done")], loaded));
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ preview: "live", output: "done", status: "ok" });
  });

  it("keeps two live calls under one id apart while both run", () => {
    const [a, b] = tools(
      foldEvents([started("x", "a"), started("x", "b"), finished("x", "first"), finished("x", "second")], []),
    );
    expect(a.id).not.toBe(b.id);
    expect([a.output, b.output]).toEqual(["first", "second"]);
  });

  it("reports to the delegation still owed one, past a later call under its id", () => {
    const spawn = { type: "tool_call_started", id: "ptc_1", name: "spawn_subagent", preview: "d" } as never;
    const report = {
      type: "delegate_report",
      id: "ptc_1",
      report: { disposition: "done", summary: "found it" },
    } as never;
    const [delegation, read] = tools(
      [spawn, finished("ptc_1", "started"), started("ptc_1", "b"), report].reduce(reduce, [] as Entry[]),
    );
    expect(delegation.report).toBeDefined();
    expect(read.report).toBeUndefined();
  });

  it("settles one of a delegate's calls per finish", () => {
    const step = (body: object) =>
      ({ type: "delegate_activity", id: "d", step: { at: 1, name: "r", preview: "p", call: "x", ...body } }) as never;
    const agents = [
      step({ kind: "tool_started" }),
      step({ kind: "tool_finished", ok: true }),
      step({ kind: "tool_started" }),
      step({ kind: "tool_finished", ok: false }),
    ].reduce(mergeAgents, NO_AGENTS);
    expect(agents.d.tools.map((t) => t.ok)).toEqual([true, false]);
  });
});

describe("two delegations under one id", () => {
  const spawn = (id: string, name: string) =>
    ({ type: "tool_call_started", id, name: "spawn_subagent", preview: name }) as never;
  const said = (id: string, text: string) =>
    ({ type: "delegate_activity", id, step: { kind: "text", text } }) as never;
  const done = (id: string) => ({ type: "tool_call_finished", id, ok: true, output: "x" }) as never;

  it("keeps a record each while watched, even inside one batch", () => {
    const { entries, agents } = foldTurn(
      [spawn("d", "first"), said("d", "one"), done("d"), spawn("d", "second"), said("d", "two")],
      [],
      NO_AGENTS,
    );
    const [a, b] = tools(entries);
    expect(a.id).not.toBe(b.id);
    expect(agents[a.id].text).toBe("one");
    expect(agents[b.id].text).toBe("two");
  });

  it("keeps a record each across batches, and past a read under the id", () => {
    let state = foldTurn([spawn("d", "first"), said("d", "one"), done("d")], [], NO_AGENTS);
    state = foldTurn(
      [{ type: "tool_call_started", id: "d", name: "read_file", preview: "r" } as never, spawn("d", "second"), said("d", "two")],
      state.entries,
      state.agents,
    );
    const cards = tools(state.entries);
    expect(state.agents[cards[0].id].text).toBe("one");
    expect(state.agents[cards[2].id].text).toBe("two");
    expect(state.agents[cards[1].id]).toBeUndefined();
  });

  it("gives a late report to the background delegation owed it", () => {
    const report = { type: "delegate_report", id: "d", report: { disposition: "done", summary: "s" } } as never;
    const { entries, agents } = foldTurn(
      [spawn("d", "bg"), { type: "tool_detached", id: "d" } as never, done("d"), spawn("d", "next"), done("d"), report],
      [],
      NO_AGENTS,
    );
    const [bg, next] = tools(entries);
    expect(agents[bg.id].report).toBeDefined();
    expect(bg.report).toBeDefined();
    expect(next.report).toBeUndefined();
  });

  it("leaves an event with no card under its own id", () => {
    const { agents } = foldTurn([said("d", "early")], [], NO_AGENTS);
    expect(agents.d.text).toBe("early");
  });
});

describe("two delegations under one id, reopened", () => {
  it("restores each transcript to its own card's record", async () => {
    const spawned = (path: string) =>
      ({ role: "assistant", content: [{ type: "tool_use", id: "d", name: "spawn_subagent", input: { path } }] }) as unknown as Message;
    const meta = (id: string, started: number) =>
      ({ id, call: "d", agent: "x", model: "m", started, updated: started }) as SessionMeta;
    const entries = linkDelegates(
      entriesFromMessages([spawned("a"), results(["d", "x"]), spawned("b"), results(["d", "y"])]),
      [meta("child-2", 2), meta("child-1", 1)],
    );
    const delegates = [meta("child-1", 1), meta("child-2", 2)];
    useStore.setState({ session: { id: "s" } as never, entries, delegates, agents: NO_AGENTS });
    invoke.mockImplementation((_: string, raw: unknown) =>
      Promise.resolve([said2(`from ${(raw as { subagentId: string }).subagentId}`)]),
    );

    await useStore.getState().restoreAgents();

    const [a, b] = tools(entries);
    expect(useStore.getState().agents[a.id].text).toBe("from child-1");
    expect(useStore.getState().agents[b.id].text).toBe("from child-2");
    expect(a.id).not.toBe(b.id);
  });
});

describe("a turn being watched", () => {
  it("files what it hears about a repeated delegation under the right card", async () => {
    let push: (event: unknown) => void = () => {};
    invoke.mockImplementation((command: string, raw: unknown) => {
      if (command === "attach_session") {
        const channel = Object.values(raw as Record<string, unknown>).find(
          (v) => typeof v === "object" && v !== null,
        ) as { onmessage: (event: unknown) => void };
        push = (event) => channel.onmessage(event);
        return Promise.resolve({ turn: null, unattended: false, dropped: 0 });
      }
      return Promise.resolve([]);
    });
    useStore.setState({ session: { id: "s" } as never, entries: [], agents: NO_AGENTS });
    await useStore.getState().attach("s");

    const spawn = { type: "tool_call_started", id: "d", name: "spawn_subagent", preview: "x" };
    push(spawn);
    push({ type: "delegate_activity", id: "d", step: { kind: "text", text: "one" } });
    push({ type: "tool_call_finished", id: "d", ok: true, output: "x" });
    push(spawn);
    push({ type: "delegate_activity", id: "d", step: { kind: "text", text: "two" } });

    await vi.waitFor(() => expect(tools(useStore.getState().entries)).toHaveLength(2));
    const [a, b] = tools(useStore.getState().entries);
    expect(useStore.getState().agents[a.id].text).toBe("one");
    expect(useStore.getState().agents[b.id].text).toBe("two");
  });
});

describe("the rule `pair_tool_results` follows", () => {
  const card = (id: string, name: string): Entry =>
    ({ kind: "tool", id, call: "x", name, preview: name, status: "running", steps: [] }) as Entry;
  const words = { kind: "assistant", id: "t", text: "hm", thinking: "", open: false } as Entry;

  it("lets the batch decide on a rebuild, past an entry that isn't a card", () => {
    const entries = [card("a", "a"), words, card("b", "b")];
    expect(answering(entries, "x", () => true)).toBe(0);
    expect(answering(entries, "x", (i) => i !== 0)).toBe(2);
  });

  it("keeps the live batch to a run of cards", () => {
    expect(answering([card("a", "a"), words, card("b", "b")], "x")).toBe(2);
    expect(answering([card("a", "a"), card("b", "b")], "x")).toBe(0);
  });

  it("answers one message's calls in order when the message has words between them", () => {
    const message = {
      role: "assistant",
      content: [
        { type: "tool_use", id: "x", name: "read_file", input: { path: "a" } },
        { type: "text", text: "and then" },
        { type: "tool_use", id: "x", name: "read_file", input: { path: "b" } },
      ],
    } as unknown as Message;
    const [a, b] = tools(entriesFromMessages([message, results(["x", "of a"], ["x", "of b"])]));
    expect([a.output, b.output]).toEqual(["of a", "of b"]);
  });

  it("doesn't reach past a notice into an earlier message's call", () => {
    const [old, next] = tools(
      entriesFromMessages(
        [calls(["x", "old"]), calls(["x", "new"]), results(["x", "of new"])],
        [{ after: 1, from_model: "a", to_model: "b" } as never],
      ),
    );
    expect(old.status).toBe("running");
    expect(next.output).toBe("of new");
  });
});

describe("a delegate's calls under one id", () => {
  const failed = (id: string) => {
    const message = results([id, "no"]);
    (message.content[0] as { is_error: boolean }).is_error = true;
    return message;
  };
  const meta = { id: "s", model: "m", started: 0, updated: 0 } as SessionMeta;

  it("settles one message's in order when rebuilt", () => {
    const run = runFromMessages("c", meta, [calls(["x", "a"], ["x", "b"]), failed("x")]);
    expect(run.tools.map((t) => t.ok)).toEqual([false, undefined]);
  });

  it("settles the nearer round's when an older call was left unanswered", () => {
    const run = runFromMessages("c", meta, [calls(["x", "a"]), calls(["x", "b"]), failed("x")]);
    expect(run.tools.map((t) => t.ok)).toEqual([undefined, false]);
  });

  const step = (body: object) =>
    ({ type: "delegate_activity", id: "d", step: { at: 1, name: "r", preview: "p", call: "x", ...body } }) as never;

  it("settles one round's in order when watched", () => {
    const agents = [
      step({ kind: "round", n: 1 }),
      step({ kind: "tool_started" }),
      step({ kind: "tool_started" }),
      step({ kind: "tool_finished", ok: false }),
    ].reduce(mergeAgents, NO_AGENTS);
    expect(agents.d.tools.map((t) => t.ok)).toEqual([false, undefined]);
  });

  it("settles the nearer round's when watched, too", () => {
    const agents = [
      step({ kind: "round", n: 1 }),
      step({ kind: "tool_started" }),
      step({ kind: "round", n: 2 }),
      step({ kind: "tool_started" }),
      step({ kind: "tool_finished", ok: false }),
    ].reduce(mergeAgents, NO_AGENTS);
    expect(agents.d.tools.map((t) => t.ok)).toEqual([undefined, false]);
  });
});
