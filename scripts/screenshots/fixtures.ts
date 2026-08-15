/**
 * The conversation the README's screenshots show.
 *
 * Written as the `UiEvent` stream a turn actually emits, and folded by the
 * store's own `reduce`, so nothing here builds view state by hand — the images
 * come out of the same function a live turn drives.
 *
 * The live stream rather than a saved transcript, because they do not look the
 * same and only one of them is typical. A tool call's row label is composed by
 * the tool in Rust and never written to disk, so a conversation reopened from
 * disk falls back to printing the raw arguments. That is honest, but it is what
 * you see after restarting the app rather than what you see while working, and
 * a screenshot should show the latter.
 */

/**
 * Seconds since the epoch, read at capture time.
 *
 * Deliberately not frozen. The rail groups conversations into Today and
 * Earlier and prints a date beside each, so a fixed timestamp would file every
 * one of them under Earlier and stamp the images with a day that recedes
 * further into the past with every release. Relative times cost a diff in the
 * PNGs whenever someone regenerates, which is rare and deliberate; a frozen one
 * costs a screenshot that looks abandoned.
 */
const NOW = Math.floor(Date.now() / 1000);

export const STATUS = {
  workspace: "/Users/you/code/taurus",
  providers: [
    {
      id: "ollama",
      kind: "ollama",
      base_url: "http://localhost:11434",
      models: [],
      default_model: "qwen3.6:27b",
      api_key_env: null,
      api_key_header: null,
      native_tools: null,
      context_length: null,
      api_prefix: null,
    },
  ],
  settings: {
    last_workspace: "/Users/you/code/taurus",
    last_provider: "ollama",
    last_model: "qwen3.6:27b",
    skill_synthesis_enabled: true,
    agent_synthesis_enabled: true,
    disabled_tools: [],
    theme: "dark",
  },
  skill_count: 12,
  agent_count: 4,
  problems: [],
  tool_names: [],
  mcp_servers: [],
};

export const SESSIONS = [
  {
    id: "s1",
    workspace: STATUS.workspace,
    model: "qwen3.6:27b",
    started: NOW - 400,
    updated: NOW - 60,
    title: "Where is the build time going?",
  },
  {
    id: "s2",
    workspace: STATUS.workspace,
    model: "qwen3.6:27b",
    started: NOW - 10_800,
    updated: NOW - 9_000,
    title: "Summarize the crates",
  },
  {
    id: "s3",
    workspace: STATUS.workspace,
    model: "qwen3.6:27b",
    started: NOW - 180_000,
    updated: NOW - 176_000,
    title: "Add the context-length field",
  },
];

export const MODELS = [
  { id: "qwen3.6:27b", display_name: "qwen3.6:27b", context_length: 32_768 },
  { id: "gemma4:12b", display_name: "gemma4:12b", context_length: 8_192 },
];

export const CHECKPOINTS = [
  {
    turn: 1,
    prompt: "Where is the build time going?",
    at: NOW - 120,
    files: ["Cargo.toml", "crates/taurus-core/src/agent.rs"],
  },
];

const TABLE = {
  title: "Crates by build time",
  caption: "From cargo build --timings, release profile.",
  columns: [
    { label: "Crate", kind: "text" },
    { label: "Time", kind: "number" },
    { label: "Units", kind: "number" },
    { label: "Δ", kind: "delta" },
  ],
  rows: [
    ["taurus-core", "42.1s", "318", "-8%"],
    ["taurus-tauri", "31.7s", "204", "+3%"],
    ["taurus-mcp", "18.4s", "96", "—"],
    ["taurus-agents", "11.9s", "61", "+22%"],
    ["taurus-skills", "6.2s", "34", "-2%"],
  ],
};

const CHART = {
  title: "Tool calls per turn",
  caption: "Last 8 turns in this conversation.",
  labels: ["t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8"],
  series: [
    { name: "tool calls", unit: "", values: [4, 7, 3, 11, 6, 9, 14, 8] },
    { name: "tokens", unit: "k", values: [12, 19, 8, 31, 17, 24, 38, 21] },
    { name: "seconds", unit: "s", values: [9, 14, 6, 26, 12, 18, 33, 15] },
  ],
};

const QUESTIONS = {
  questions: [
    {
      prompt: "Which of these should I dig into?",
      kind: "single",
      allow_other: false,
      options: [
        { label: "taurus-core — the long pole", note: "42.1s" },
        { label: "taurus-agents — the regression", note: "+22%" },
        { label: "Both, in that order", note: "slower" },
      ],
    },
    {
      prompt: "How far may I go on my own?",
      kind: "multi",
      allow_other: true,
      options: [
        { label: "Split the crate", note: "invasive" },
        { label: "Trim its dependencies", note: "safe" },
        { label: "Only measure, change nothing", note: "" },
      ],
    },
  ],
};

const say = (text: string) => ({ type: "text_delta", text });

/** `preview` is the line the tool itself composes, copied from its Rust `preview`. */
const call = (id: string, name: string, preview: string, view?: unknown) => ({
  type: "tool_call_started",
  id,
  name,
  preview,
  ...(view ? { view } : {}),
});

const done = (id: string, output: string) => ({
  type: "tool_call_finished",
  id,
  ok: true,
  output,
});

/**
 * What the user asked. Not an event — the store appends this itself when the
 * message is sent, so the harness has to as well.
 */
export const PROMPT = "Where is the build time going? Show me the shape of it.";

/** One turn: real work, a table, a chart, and a question left open. */
export const EVENTS = [
  say("Timing a clean release build now."),
  call("c1", "run_command", "Run: cargo build --release --timings"),
  done("c1", "Finished `release` profile in 110.3s"),
  call("c2", "read_file", "Read target/cargo-timings/cargo-timing.html"),
  done("c2", "…"),
  say(
    "110.3s in total, and it is concentrated rather than spread: `taurus-core` alone is over a third of it.",
  ),
  call("c3", "show_table", "Table: Crates by build time", {
    type: "table",
    ...TABLE,
  }),
  done("c3", "Drew 'Crates by build time' — 5 rows over 4 columns."),
  say(
    "The shape per turn is spikier than the totals suggest — turn 7 costs nearly five times turn 3.",
  ),
  call("c4", "show_chart", "Chart: Tool calls per turn", {
    type: "chart",
    ...CHART,
  }),
  done("c4", "Drew 'Tool calls per turn' — 8 bars across 3 series."),
  say(
    "`taurus-core` is the long pole and `taurus-agents` is the one that got worse. Which is worth my time?",
  ),
  // Left unfinished on purpose: the card is only answerable while the call it
  // belongs to is still running, and that is the state worth photographing.
  call("c5", "ask_user", "Ask 2 questions", {
    type: "questions",
    id: "c5",
    ...QUESTIONS,
  }),
];
