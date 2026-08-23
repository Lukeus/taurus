# Taurus AI Shell

An agent harness that runs against any model provider — local Ollama, anything
OpenAI-compatible, Anthropic, or Google Gemini. Rust underneath, with two
frontends over one shared core: a Tauri v2 desktop app and a `taurus` CLI.
macOS, Windows, and Linux from one codebase.

It reads and edits files in a workspace, runs commands, searches the web,
connects to MCP servers, delegates to sub-agents, reads screenshots you paste in,
leaves itself notes so the next conversation in a workspace does not start from
nothing,
and finds code by what it does rather than what it is called — and writes down
procedures it works out as reusable **skills**, which you approve before they are
kept. It
reads the `AGENTS.md` and `CLAUDE.md` you already have rather than asking for a
seventh copy. Every file it edits is recorded first, so any turn can be
**rewound** — or read back as a diff and **committed on its own** — and every
write is shown as a diff before you approve it.

![The Taurus desktop app: a conversation, a folded run of tool calls, and a
table the model drew](docs/screenshots/app-dark.png)

A turn folds its tool calls into one card, so a nine-step turn reads as one step
of the conversation. Results the model means you to *look* at — a table, a chart
— stand on their own beside the prose rather than inside that card.

![The same app in its light theme, showing a bar chart with a tab per series and
a question card below it](docs/screenshots/app-light.png)

When a decision is genuinely yours, it asks and waits rather than guessing. Every
question can be skipped — "You decide" answers all of them at once — so the turn
never blocks on an answer that is not coming.

![A question card in the transcript, with single-choice and multiple-choice
questions and a free-text box](docs/screenshots/questions.png)

<sub>Regenerate with `pnpm screenshots`. These are the real interface driven by
fixtures in headless Chrome rather than photographs of a running window — see
[`scripts/screenshots/`](scripts/screenshots/capture.mjs) for why, and for what
that costs.</sub>

## Quick start

```bash
ollama serve                # in another terminal
```

Desktop app:

```bash
pnpm install
pnpm tauri dev
```

Or the CLI:

```bash
cargo install --path crates/taurus-cli

taurus repl                                     # interactive
taurus run "summarize the modules in src/"      # one-shot
taurus run --json "count the rust files" | jq   # for scripts
taurus rewind --to last                         # undo what the last turn wrote
```

Both share `~/.taurus` — same providers, same skills, same permission
allowlist. Approve a tool once in the app and your scripted runs inherit it.

## What it does

Each of these has a section of its own. The one-liners are here so you can tell
from this page whether the thing you want exists.

**[Capabilities](docs/capabilities.md)** — what it reaches for, and what it
writes down.

- [**Instructions**](docs/capabilities.md#instructions) — reads the `AGENTS.md`
  and `CLAUDE.md` you already have, rather than asking for a seventh copy.
- [**Skills**](docs/capabilities.md#skills) — works a procedure out once, writes
  it down, and asks before keeping it. You approve; nothing is saved behind your
  back.
- [**Memory**](docs/capabilities.md#memory) — writes down what the next
  conversation in this workspace would otherwise have to be told again. You can
  read every note, and forget any of them.
- [**Sub-agents**](docs/capabilities.md#sub-agents) — delegates to a scoped
  context with its own tools, so a search that would fill the window happens
  somewhere else.
- [**Slash commands**](docs/capabilities.md#slash-commands) — one namespace over
  skills, sub-agents, and built-ins.

**[Permission, and undo](docs/safety.md)** — what it asks before acting, and how
to put things back after.

- [**Permissions**](docs/safety.md#permissions) — every write is shown as a diff
  before you approve it, and a decision can be remembered for this project or
  everywhere.
- [**Hooks**](docs/configuration.md#hooks) — your own programs run at fixed
  points in a turn, able to refuse a call and never to approve one.
- [**Trusting a workspace**](docs/safety.md#trusting-a-workspace) — a cloned
  repository's own config does not configure your agent until you say so, and
  you are only asked when the folder actually holds something.
- [**Running commands**](docs/safety.md#running-commands) — a real PTY per
  platform, so a program that checks `isatty` behaves the way it does in a
  terminal.
- [**Rewinding a turn**](docs/safety.md#rewinding-a-turn) — every file a turn
  touched is recorded first, so any turn can be undone.
- [**Keeping a turn**](docs/safety.md#keeping-a-turn) — read a turn back as a
  diff and commit it on its own.

**[Working with it](docs/working-with-it.md)** — what a turn looks like in use.

- [**Sessions**](docs/working-with-it.md#sessions) — transcripts on disk, per
  workspace, replayable.
- [**Planning a long task**](docs/working-with-it.md#planning-a-long-task) — a
  plan it pins and keeps current, rather than one it announces once and forgets.
- [**Showing it a picture**](docs/working-with-it.md#showing-it-a-picture) —
  paste a screenshot in.
- [**Finding code by what it does**](docs/working-with-it.md#finding-code-by-what-it-does)
  — local semantic search, no service.
- [**When a turn stops**](docs/working-with-it.md#when-a-turn-stops), and
  [**the context window**](docs/working-with-it.md#the-context-window) — what
  ends a turn, and what it costs.
- [**Tables, charts, and questions**](docs/working-with-it.md#tables-charts-and-questions)
  — results you are meant to *look* at stand on their own beside the prose. When
  a decision is genuinely yours, it asks and waits — and every question can be
  skipped.
- [**Working with data**](docs/working-with-it.md#working-with-data) — a CSV
  with a million rows in it is not a file to read. It loads one as a table
  instead, describes every column from the whole file rather than a sample, and
  puts the rows on a surface of their own.

![The Data pane: a profile of a 400,000-row file, with the missing values
marked](docs/screenshots/data.png)

The pane does not exist until a workspace has loaded something. It takes the
centre column beside the conversation rather than covering it, and the box you
type in never moves — asking is still how anything gets here.

**[Configuration](docs/configuration.md)** — providers, keys, MCP servers, and
web search.

- Local Ollama, anything OpenAI-compatible, Anthropic, or Google Gemini.
- Keys live in the OS keychain or an env var, never in a config file.
- Everything the Settings drawer writes is a plain file the CLI reads too.
- [**MCP servers**](docs/configuration.md#mcp-servers) — add and test them in
  the app, in the same `mcpServers` format Claude Desktop uses.

![The MCP panel](docs/screenshots/mcp.png)

**[Development](docs/development.md)** — tests, the live checks, the app icon,
regenerating these screenshots, and cutting a release.

**[Known gaps](docs/known-gaps.md)** — what it does not do, written down where
you can read it before you discover it.

## How it is put together

```
crates/
  taurus-provider/          Provider trait + normalized message/stream types
  taurus-provider-ollama/   Ollama adapter (NDJSON, per-model capabilities)
  taurus-provider-openai/   OpenAI-compatible adapter (SSE, vLLM/LM Studio/…)
  taurus-provider-anthropic/ Anthropic Messages API (probed capabilities, caching)
  taurus-provider-gemini/   Google Gemini (generateContent, OpenAPI-subset schemas)
  taurus-tools/             Tool registry, built-in tools, permission gate, undo
  taurus-skills/            Skill discovery, execution, and authoring
  taurus-agents/            Sub-agent definitions and discovery
  taurus-mcp/               MCP client
  taurus-web/               Web search and page fetching
  taurus-index/             Local semantic search: chunking, embedding, ranking
  taurus-data/              Reading and profiling tabular files, behind one engine trait
  taurus-core/              Session state, the agent loop, sub-agents
  taurus-host/              Config, system prompt, registry assembly
  taurus-cli/               The `taurus` binary
src-tauri/                  Windows and IPC — no agent logic
src/                        React UI
```

One rule holds the design together: **a frontend contains no agent logic.**
Everything a session *is* — config files, system prompt, tool registry, skill
library — lives in `taurus-host`, and `Host::build_agent` is the single place
they come together. The desktop app and the CLI differ only in how they talk to
a person: how a permission prompt is asked, and where a skill proposal goes.
Both are traits.

That is also why the agent loop can be tested against a scripted provider, and
why the [live checks](docs/development.md#live-checks) run without a GUI.

### Provider-agnostic, concretely

The normalized types use Anthropic-style content blocks rather than OpenAI's
`tool_calls` shape, because blocks are the superset — a single assistant turn
with interleaved text, reasoning, and several tool calls survives the round
trip; the other direction does not.

Three things prove the abstraction rather than assert it:

- **Every adapter after the first required no change to `taurus-core`.** The
  OpenAI one differs in transport (SSE vs NDJSON) and tool-call encoding
  (arguments as a string assembled across frames vs a whole object). Gemini
  differs in what a conversation *is*: the assistant is called `model`, tool
  calls carry no ids at all, results pair with calls by name, schemas are an
  OpenAPI subset rather than JSON Schema, and every streamed chunk is a whole
  response object rather than a delta envelope. None of that reached the core.
- **Models without tool support still call tools.** `gemma3` accepts no `tools`
  parameter at all. The harness detects that from Ollama's capability probe and
  switches to prompted tool calling, parsing `<tool_call>` blocks out of the
  text stream into the exact same events a native adapter emits. `taurus-core`
  cannot tell which path a turn took.
- **A backend that reports itself is asked rather than configured.** Ollama and
  Anthropic both answer questions about their own models, so neither needs a
  `context_length` in `providers.json` and neither can be told the wrong one.

## License

MIT. See [LICENSE](LICENSE).
