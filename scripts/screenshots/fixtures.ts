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
  problems: [
    {
      source: "mcp",
      message:
        "mcp server 'notion' has no `command` or `url`, and does not recognise `commnd` (did you mean `command`?)",
    },
  ],
  tool_names: [],
  // Only the counts the rail draws. The panel asks `list_mcp_servers` for the
  // rest, which is what `MCP_SERVERS` below answers.
  mcp_servers: [
    { name: "filesystem", connected: true },
    { name: "github", connected: true },
    { name: "linear", connected: false },
    { name: "postgres", connected: false },
  ],
};

/**
 * The panel's own listing: one of each state worth photographing.
 *
 * Deliberately not four healthy servers. The panel exists for the ones that are
 * not working, and a picture of it that shows only green says nothing about
 * what it is for.
 */
export const MCP_SERVERS = [
  {
    name: "filesystem",
    scope: "global",
    transport: "stdio",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-filesystem", "/Users/you/code"],
    env: [],
    url: "",
    headers: [],
    disabled: false,
    program: "/Users/you/.nvm/versions/node/v22.17.1/bin/npx",
    status: {
      name: "filesystem",
      description: "npx -y @modelcontextprotocol/server-filesystem",
      connected: true,
      tool_count: 14,
      error: null,
      disabled: false,
      tools: ["read_file", "write_file", "edit_file", "list_directory"],
    },
  },
  {
    name: "github",
    scope: "workspace",
    transport: "http",
    command: "",
    args: [],
    env: [],
    url: "https://api.githubcopilot.com/mcp/",
    headers: [{ key: "Authorization", value: "", secret: true }],
    disabled: false,
    status: {
      name: "github",
      description: "https://api.githubcopilot.com/mcp/",
      connected: true,
      tool_count: 26,
      error: null,
      disabled: false,
      tools: ["create_issue", "search_code", "get_pull_request"],
    },
  },
  {
    // The failure this whole feature was rebuilt for.
    name: "linear",
    scope: "global",
    transport: "stdio",
    command: "uvx",
    args: ["linear-mcp-server"],
    env: [],
    url: "",
    headers: [],
    disabled: false,
    program: null,
    status: {
      name: "linear",
      description: "uvx linear-mcp-server",
      connected: false,
      tool_count: 0,
      error:
        "`uvx` is not on this application's PATH. It searched: /usr/bin:/bin:/usr/sbin:/sbin.",
      disabled: false,
      tools: [],
    },
  },
  {
    name: "postgres",
    scope: "workspace",
    transport: "stdio",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-postgres"],
    env: [{ key: "DATABASE_URL", value: "${DATABASE_URL}", secret: false }],
    url: "",
    headers: [],
    disabled: true,
    program: "/Users/you/.nvm/versions/node/v22.17.1/bin/npx",
    status: {
      name: "postgres",
      description: "npx -y @modelcontextprotocol/server-postgres",
      connected: false,
      tool_count: 0,
      error: null,
      disabled: true,
      tools: [],
    },
  },
];

/** What the panel says about where it looks for a stdio server's program. */
export const MCP_ENVIRONMENT = {
  path: [
    "/Users/you/.nvm/versions/node/v22.17.1/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
  ],
  added: ["/Users/you/.nvm/versions/node/v22.17.1/bin", "/opt/homebrew/bin"],
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
/**
 * The moment the diff exists for: a write that replaces lines already on disk.
 *
 * A byte count would say a file is about to change and nothing about what to,
 * which is the whole reason this view was added — so the fixture is an
 * overwrite rather than a creation.
 */
export const PERMISSION = {
  id: "perm-1",
  tool: "edit_file",
  effect: "write",
  preview: "Edit crates/taurus-core/src/agent.rs",
  always_scope: "Allows edit_file in this workspace",
  always_global_scope: "Allows edit_file in every workspace",
  input: {},
  diff: {
    path: "crates/taurus-core/src/agent.rs",
    created: false,
    added: 2,
    removed: 1,
    elided: 0,
    // One line rewritten and one line added, which is what an edit usually is
    // — and the two halves of the diff view in one hunk. The rewritten pair is
    // a removal answered by exactly one addition, so the characters that
    // differ are marked inside them; the `compact_if_needed` line is an
    // addition answering nothing, so it is not paired and nothing in it is
    // marked. An earlier version of this fixture had the *same* line on both
    // sides, which no diff produces.
    hunks: [
      {
        lines: [
          { kind: "context", text: "    let mut usage = TokenUsage::default();", old_line: 411, new_line: 411 },
          { kind: "context", text: "", old_line: 412, new_line: 412 },
          { kind: "removed", text: "    for round in 0..MAX_ITERATIONS {", old_line: 413, new_line: null },
          { kind: "added", text: "    for round in 0..self.config.max_iterations {", old_line: null, new_line: 413 },
          { kind: "context", text: "        let request = self.build_request();", old_line: 414, new_line: 414 },
          { kind: "added", text: "        self.compact_if_needed(&mut usage).await;", old_line: null, new_line: 415 },
          { kind: "context", text: "        let (tx, rx) = mpsc::channel(64);", old_line: 415, new_line: 416 },
        ],
      },
    ],
  },
} as const;

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

/**
 * The same conversation, caught mid-run.
 *
 * `EVENTS` ends on a question card, which is a turn parked rather than a turn
 * working — so this drops it and leaves a write in flight instead. What the
 * shot is of is the motion: the waveform under the thread taking the shape of
 * the category that is running, and the running row wearing the gutter bar
 * that goes with a write.
 */
export const MOTION_EVENTS = [
  // Two dropped, not one: `EVENTS` ends on a sentence *and* the question card
  // it leads into, and consecutive text events fold into a single bubble — so
  // appending after only the card ran two sentences together with no space.
  ...EVENTS.slice(0, -2),
  say("Pulling the iteration cap out of the loop so it can be configured."),
  call("m1", "edit_file", "Edit crates/taurus-core/src/agent.rs"),
];

/*
 * A dataset, profiled.
 *
 * The numbers are a real run of `cargo run -p taurus-data --example probe` over
 * a 400,000-row interactions file, not invented ones — which matters for the
 * two columns the picture is actually of: `rating`, 42% missing, and `user_id`,
 * with too many distinct values for a top five to mean anything. A frame of
 * seven clean columns would say nothing about what this pane is for.
 */
/**
 * A short data conversation, for the shot of the cards it leaves behind.
 *
 * Separate from `EVENTS`, which is a build-timing conversation and has nothing
 * to load or query. The point of this one is the three cards a data turn
 * produces side by side — two references into the pane and one query you can
 * take back out of it — which is the whole of the round trip in one frame.
 *
 * The numbers are the same 400,000-row interactions file the profile below is
 * a real run over, so the sentences agree with `data.png`.
 */
export const DATA_PROMPT = "Load data/interactions.csv and tell me which category people actually finish.";

export const DATA_EVENTS = [
  say("Loading it now."),
  call("d1", "load_dataset", "Load data/interactions.csv", {
    type: "dataset",
    name: "interactions",
  }),
  done("d1", "Loaded 'interactions' — 400,000 rows over 6 columns."),
  say(
    "400,000 rows over six columns. `rating` is 42% missing, so anything about satisfaction has to say which rows it is over.",
  ),
  call("d2", "query_data", "Query: SELECT category, count(*) AS n, avg(rating)…", {
    type: "query",
    sql:
      "SELECT category,\n       count(*) AS n,\n       round(avg(rating), 2) AS rated\n  FROM interactions\n WHERE rating IS NOT NULL\n GROUP BY category\n ORDER BY rated DESC",
  }),
  done("d2", "5 rows.\n\ncategory  n       rated\nbooks     41,208  4.31\nkitchen   28,904  4.02"),
  say(
    "`books` finishes highest at 4.31 and `kitchen` is close behind — but both are over the 58% of rows that carry a rating at all.",
  ),
];

/**
 * What the query above answers with, for the other half of the round trip.
 *
 * The same five categories the sentence after the card names, so the pane's
 * grid and the model's prose agree — a picture where they did not would be a
 * picture of a bug.
 */
export const DATA_QUERY = {
  columns: [
    { name: "category", kind: "text", type_name: "Utf8", nullable: false },
    { name: "n", kind: "number", type_name: "Int64", nullable: false },
    { name: "rated", kind: "number", type_name: "Float64", nullable: true },
  ],
  rows: [
    ["books", "41208", "4.31"],
    ["kitchen", "28904", "4.02"],
    ["outdoors", "19430", "3.88"],
    ["electronics", "44117", "3.51"],
    ["toys", "12094", "3.44"],
  ],
  truncated: false,
  took_ms: 63,
};

export const DATASETS = [
  { name: "interactions", path: "data/interactions.csv", format: "csv" },
  { name: "items", path: "data/items.parquet", format: "parquet" },
];

const column = (
  name: string,
  type_name: string,
  kind: string,
  over: Record<string, unknown> = {},
) => ({
  head: { name, kind, type_name, nullable: true },
  nulls: 0,
  distinct: { kind: "exact", count: 0 },
  min: null,
  max: null,
  common: [],
  ...over,
});

const share = (value: string | null, count: number) => ({ value, count });

export const DATA_PROFILE = {
  rows: 400_000,
  engine: "DataFusion",
  columns: [
    column("user_id", "Utf8", "text", {
      distinct: { kind: "exact", count: 49_981 },
      min: "u1",
      max: "u9999",
    }),
    column("item_id", "Utf8", "text", {
      distinct: { kind: "exact", count: 9_000 },
      min: "i1",
      max: "i8999",
    }),
    column("event", "Utf8", "text", {
      distinct: { kind: "exact", count: 5 },
      min: "add_to_cart",
      max: "view",
      common: [
        share("view", 219_922),
        share("click", 100_189),
        share("add_to_cart", 47_743),
      ],
    }),
    column("category", "Utf8", "text", {
      distinct: { kind: "exact", count: 6 },
      min: "apparel",
      max: "toys",
      common: [share("electronics", 67_179), share("apparel", 67_102)],
    }),
    column("price", "Float64", "number", {
      nulls: 12_004,
      distinct: { kind: "exact", count: 138_692 },
      min: "1.00",
      max: "1498.99",
    }),
    column("rating", "Int64", "number", {
      nulls: 167_853,
      distinct: { kind: "exact", count: 5 },
      min: "1",
      max: "5",
      common: [share(null, 167_853), share("4", 46_544), share("1", 46_503)],
    }),
    // 336 distinct is under the ceiling, so this one *does* get a top five —
    // leaving it empty in the fixture drew "too many values to rank" beside a
    // three-figure count, which is the pane contradicting itself.
    column("ts", "Timestamp(s)", "temporal", {
      distinct: { kind: "exact", count: 336 },
      min: "2024-01-01T10:00:00",
      max: "2024-12-28T10:00:00",
      common: [
        share("2024-04-27T10:00:00", 1_283),
        share("2024-08-23T10:00:00", 1_278),
      ],
    }),
  ],
};

/*
 * What the query box knows about the tables it can name.
 *
 * The same columns the profile above reports, minus everything the profile had
 * to read the file to find out — which is the difference between the two calls
 * and the reason completion can afford this one. `items` is a second file
 * sharing `item_id` and `category`, because a workspace with one table has
 * nothing to demonstrate: the join marks in the list and in the schema panel
 * are the whole point of both.
 */
export const DATA_TABLES = [
  {
    name: "interactions",
    path: "data/interactions.csv",
    // A CSV keeps no count, so there is none to give without reading it.
    rows: null,
    columns: DATA_PROFILE.columns.map((c) => c.head),
  },
  {
    name: "items",
    path: "data/items.parquet",
    rows: 9_000,
    columns: [
      { name: "item_id", kind: "text", type_name: "Utf8", nullable: false },
      { name: "title", kind: "text", type_name: "Utf8", nullable: false },
      { name: "category", kind: "text", type_name: "Utf8", nullable: true },
      { name: "list_price", kind: "number", type_name: "Float64", nullable: true },
      { name: "in_stock", kind: "boolean", type_name: "Boolean", nullable: false },
    ],
  },
];

/*
 * A recipe, and a run of it.
 *
 * The numbers are a real `taurus data run` over the same 400,000-row file the
 * profile above came from, not invented ones — and the reason to photograph
 * this view rather than the recipe sitting still is the delta column. A step
 * that dropped 375,968 rows is the whole argument for reporting per step, and
 * a frame of four rows all saying "done" would say nothing about it.
 */
export const RECIPES = {
  recipes: [
    {
      name: "purchases",
      source: "data/interactions.csv",
      output: "data/purchases.parquet",
      description: "the purchases, deduplicated, rated, and ranked per user",
      path: ".taurus/recipes/purchases.sql",
      tables: [],
      steps: [
        { title: "drop exact duplicates", sql: "SELECT DISTINCT * FROM input" },
        {
          title: "keep the purchases",
          sql: "SELECT * FROM input WHERE event = 'purchase'",
        },
        {
          title: "drop the rows with no rating",
          sql: "SELECT * FROM input WHERE rating IS NOT NULL",
        },
        {
          title: "rank each user's purchases by price",
          sql: "SELECT user_id, item_id, category, price, rating, ts,\n       row_number() OVER (PARTITION BY user_id ORDER BY price DESC) AS rank_for_user\nFROM input",
        },
      ],
    },
    // A second one, unrun, and one that binds a table of its own — because a
    // workspace with a single recipe says nothing about what the list is for,
    // and `tables:` is the thing that makes a recipe an enrichment rather than
    // only a filter.
    {
      name: "enriched",
      source: "data/interactions.csv",
      output: "data/enriched.parquet",
      description: "interactions with each item's title and brand alongside",
      path: ".taurus/recipes/enriched.sql",
      tables: [{ name: "items", path: "data/items.parquet" }],
      steps: [
        {
          title: "attach the catalogue",
          sql: "SELECT i.*, c.title, c.brand\nFROM input i\nLEFT JOIN items c ON i.item_id = c.id",
        },
        {
          title: "drop the interactions with no item",
          sql: "SELECT * FROM input WHERE title IS NOT NULL",
        },
      ],
    },
  ],
  problems: [],
};

export const RECIPE_RUN = {
  started_with: 400_000,
  steps: [
    { title: "drop exact duplicates", rows: 400_000, columns: 7, took_ms: 517 },
    { title: "keep the purchases", rows: 24_032, columns: 7, took_ms: 240 },
    { title: "drop the rows with no rating", rows: 13_980, columns: 7, took_ms: 41 },
    {
      title: "rank each user's purchases by price",
      rows: 13_980,
      columns: 7,
      took_ms: 50,
    },
  ],
  columns: [
    { name: "user_id", kind: "text", type_name: "Utf8View", nullable: true },
    { name: "item_id", kind: "text", type_name: "Utf8View", nullable: true },
    { name: "category", kind: "text", type_name: "Utf8View", nullable: true },
    { name: "price", kind: "number", type_name: "Float64", nullable: true },
    { name: "rating", kind: "number", type_name: "Int64", nullable: true },
    { name: "ts", kind: "temporal", type_name: "Timestamp(s)", nullable: true },
    { name: "rank_for_user", kind: "number", type_name: "UInt64", nullable: false },
  ],
  rows: 13_980,
  bytes: 233_472,
  took_ms: 848,
};

/**
 * What a transcript search finds.
 *
 * Two conversations rather than one, and neither of them the one that is open:
 * the group exists to reach the conversations the rail is *not* showing you,
 * and a single hit in the session already on screen would be a picture of the
 * feature not being needed. The excerpts hold real sentences with the match
 * inside them rather than at an edge, because where the mark lands is the one
 * thing this image checks that no test can.
 */
export const SEARCH_HITS = {
  sessions: [
    {
      session: SESSIONS[2],
      hits: 4,
      matches: [
        {
          message: 6,
          role: "assistant",
          excerpt:
            "…an OpenAI-compatible endpoint cannot be asked how much a model holds, so the context length has to be written down.",
          from: 78,
          to: 85,
        },
      ],
    },
    {
      session: SESSIONS[1],
      hits: 1,
      matches: [
        {
          message: 2,
          role: "user",
          excerpt: "which crate decides the context length for a turn?",
          from: 24,
          to: 31,
        },
      ],
    },
  ],
  more: 0,
};

/**
 * A conversation's account, for the Context panel.
 *
 * `read_file` dominating is not an invention — it is what the real command
 * reports on almost every working session, and it is the finding the panel
 * exists to hand you. The repeated calls and the failure are here for the same
 * reason the MCP shot shows a broken server: a frame in which nothing is worth
 * acting on is a picture of a panel with no purpose.
 */
export const USAGE = {
  sessions: 1,
  turns: 6,
  messages: 41,
  reported_in: 312_151,
  reported_out: 3_385,
  cached_in: 214_000,
  history: 39_612,
  tools: [
    { name: "read_file", calls: 11, tokens: 34_954, failures: 0, share: 82 },
    { name: "search_code", calls: 4, tokens: 4_120, failures: 0, share: 9 },
    { name: "run_command", calls: 6, tokens: 2_180, failures: 1, share: 5 },
    { name: "grep", calls: 9, tokens: 1_040, failures: 0, share: 2 },
    { name: "list_dir", calls: 12, tokens: 604, failures: 0, share: 1 },
  ],
  repeats: 3,
  repeat_tokens: 9_400,
  system_prompt: 1_383,
  schemas: [
    { name: "propose_agent", tokens: 501 },
    { name: "draft_mcp_server", tokens: 439 },
    { name: "propose_skill", tokens: 439 },
    { name: "run_command", tokens: 429 },
    { name: "run_recipe", tokens: 404 },
    { name: "search_code", tokens: 318 },
    { name: "edit_file", tokens: 296 },
    { name: "grep", tokens: 271 },
  ],
};

/**
 * Two commands running in the background, and what one of them has printed.
 *
 * A failing test run rather than a clean one, for the reason the MCP shot
 * shows a server that is not answering: this pane exists because the model can
 * watch a build the user cannot, and a frame of green output would say nothing
 * about why anybody would look at it.
 *
 * The dev server beside it is the other half of what these are for — a command
 * that will not finish on its own, and that a `check_command` between turns is
 * the wrong way to keep an eye on.
 */
export const BACKGROUND_JOBS = [
  {
    id: 1,
    command: "cargo test --workspace",
    running: false,
    stopped: false,
    code: 101,
    ran_for: 96,
    status: "exited with code 101 after 1m36s",
  },
  {
    id: 2,
    command: "pnpm dev --host",
    running: true,
    stopped: false,
    ran_for: 214,
    status: "still running after 3m34s",
  },
];

/** What the failing run said. The tab this is under is the one on screen. */
export const BACKGROUND_OUTPUT = `   Compiling taurus-tools v0.2.0
   Compiling taurus-host v0.2.0
    Finished \`test\` profile [unoptimized + debuginfo] target(s) in 41.02s
     Running unittests src/lib.rs

running 383 tests
test jobs::tests::the_window_reads_a_command_without_taking_it_from_the_model ... ok
test jobs::tests::a_window_cursor_only_asks_for_what_it_has_not_seen ... ok
test sweep::tests::a_rename_is_one_change_and_not_two ... FAILED

failures:

---- sweep::tests::a_rename_is_one_change_and_not_two stdout ----
thread 'sweep::tests::a_rename_is_one_change_and_not_two' panicked at
crates/taurus-tools/src/sweep.rs:1204:9:
assertion \`left == right\` failed: the rename was recorded as a delete and a create
  left: 2
 right: 1

failures:
    sweep::tests::a_rename_is_one_change_and_not_two

test result: FAILED. 382 passed; 1 failed; 0 ignored; 0 measured
`;
