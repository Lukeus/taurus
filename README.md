# Taurus AI Shell

An agent harness that works with any model provider: local Ollama, anything
OpenAI-compatible, Anthropic, or Google Gemini. It's Rust underneath, with two
frontends on one shared core: a Tauri v2 desktop app and a `taurus` CLI. One
codebase covers macOS, Windows, and Linux.

Point it at a workspace and it:

- reads and edits files, runs commands, and searches the web
- connects to MCP servers and delegates to sub-agents
- reads screenshots you paste in
- leaves itself notes, so your next conversation in that workspace doesn't
  start from nothing
- finds code by what it does, not only by what it's called
- writes down procedures it works out as reusable **skills**, which you approve
  before they're kept
- reads the `AGENTS.md` and `CLAUDE.md` you already have, so you don't need a
  seventh copy

Every write shows up as a diff before you approve it. Every file it edits is
recorded first, so you can **rewind** any turn, or read it back as a diff and
**commit it on its own**.

![The Taurus desktop app: a conversation, a folded run of tool calls, and a
table the model drew](docs/screenshots/app-dark.png)

A turn folds its tool calls into one card, so a nine-step turn reads as one
step of the conversation. Results you're meant to *look* at, like a table or a
chart, sit on their own beside the text instead of inside that card.

![The same app in its light theme, showing a bar chart with a tab per series and
a question card below it, with the pill that offers the way back to the foot of
the conversation](docs/screenshots/app-light.png)

When a decision is really yours, it asks and waits instead of guessing. You can
skip any question, and "You decide" answers all of them at once, so a turn
never hangs on an answer that isn't coming.

![A question card in the transcript, with single-choice and multiple-choice
questions and a free-text box](docs/screenshots/questions.png)

<sub>Regenerate these with `pnpm screenshots`. They're the real interface,
driven by fixtures in headless Chrome, not photos of a running window. See
[`scripts/screenshots/`](scripts/screenshots/capture.mjs) for why, and what
that costs.</sub>

## Quick start

You need Rust 1.87 or newer. The desktop app also needs Node and pnpm (CI uses
Node 22 and pnpm 9) and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your
platform.

The quickest way to try it is against a local model. Start Ollama, with at
least one model pulled:

```bash
ollama serve                # in another terminal
```

Then run the desktop app:

```bash
pnpm install
pnpm tauri dev
```

Or install the CLI:

```bash
cargo install --path crates/taurus-cli

taurus repl                                     # interactive
taurus run "summarize the modules in src/"      # one-shot
taurus run --json "count the rust files" | jq   # for scripts
taurus rewind --to last                         # undo what the last turn wrote
```

Both use `~/.taurus`, so they share providers, skills, and the permission
allowlist. Approve a tool once in the app and your scripted runs inherit it.

To use a hosted model instead, see [Configuration](docs/configuration.md).

## What is different about it

Most of [What it does](#what-it-does) exists in some form in every agent
harness. These are the parts that don't. Each one comes with a limit, and
[Known gaps](docs/known-gaps.md) spells those out in full.

**It's built for the model on your own machine, not only one in a
datacenter.** Every decision here is shaped around an 8k context window:

- A fifty-skill library costs one line per skill in the prompt. A procedure
  loads only when it's needed.
- The plan is rebuilt at the *tail* of every request, not in the system
  prompt. Ticking a checkbox invalidates one line of prompt cache instead of
  the tools and the whole conversation. On the same three-step task, that took
  75 seconds against 194.
- Tool schemas are slimmed on the way out, and old tool results shrink before
  anything gets summarized.
- A model with no tool-calling API still calls tools, through prompted parsing.
  The core can't tell that path from the native one.

It won't turn a small model into a large one. It makes a small one affordable
to actually run an agent on. See
[the context window](docs/working-with-it.md#the-context-window).

**One core, two frontends, one state directory.** The desktop app and the
`taurus` CLI are the same agent. `Host::build_agent` is the only place a
session gets assembled. A frontend holds no agent logic, just the way it asks
you for permission. That means:

- A tool you approve in the app is approved for your scripted runs.
- A conversation started in one can be resumed in the other.
- The agent loop is tested against a scripted provider, with no GUI involved.

They're not identical surfaces, though. Reading a turn back as a diff and
committing it are app features, and the terminal dock is desktop-only on
purpose. See [How it is put together](#how-it-is-put-together).

**Undo covers what a command did, not only what a tool declared.** `edit_file`
can say which file it's about to change. A command line can't. So Taurus reads
the workspace before every command and again when it finishes, and the
difference is the answer. A `sed -i` across a dozen files, an `rm` that took
the wrong directory, a script the model wrote and ran: all of it comes back,
from the files as they were before it ran.

You don't need git, and it works for a command left running in the background
across turns. Read the same log forwards and you get a diff of any turn, plus
the option to commit that turn on its own.

The limit: two things can't be restored, and it tells you before you press
anything. Those are `.git`'s own state, and anything under a directory an
ignore rule excludes. See [Rewinding a turn](docs/safety.md#rewinding-a-turn).

**It reads the configuration you already have, and never writes to it.** That
includes:

- `AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`, and Copilot's
  scoped `*.instructions.md`
- skills from `.claude/skills`, `.copilot/skills`, and `.github/skills`
- sub-agents from Claude's and Copilot's own directories
- MCP servers in the `mcpServers` format Claude Desktop uses

A borrowed file is read and never rewritten. If you retune a Copilot agent,
Taurus saves its own copy beside the original, and that copy takes precedence.
The original is committed, Copilot is still reading it, and it carries keys
Taurus has never heard of.

The limit: borrowing isn't emulation. An `applyTo` glob becomes a sentence in
the prompt, because Taurus assembles the brief once per turn instead of
attaching it when a matching file is touched. See
[Instructions](docs/capabilities.md#instructions).

**A repo you just cloned doesn't get to configure your agent.** A workspace's
`.taurus` can start processes, name the endpoint your conversation is sent to,
and carry standing permission grants. All of it arrives with `git clone`.

So an untrusted workspace contributes *no* config at all. That's one rule, and
it only goes one way. You're asked only when the folder actually holds
something, and you see the MCP command lines themselves, not just a count.

The limit: it's not a sandbox. It decides whether a project can configure
Taurus, not what running that project's build script does. See
[Trusting a workspace](docs/safety.md#trusting-a-workspace).

**A data file is a surface, not a wall of text in the context window.** Handing
a million-row CSV to `read_file` is about the most expensive mistake an agent
can make. Taurus loads it as a table instead:

- It profiles every row, not a sample.
- You ask questions in SQL. Each query is planned first and refused if it does
  anything but read.
- The rows go on a pane of their own and never into a tool result.

A transformation worth keeping becomes a recipe. That's SQL committed with
your code, which you can re-run on next month's export, and which reports what
each step did to the row count.

The limit: you're committing to a dialect. A recipe is DataFusion's SQL in a
file in your repo. See
[Working with data](docs/working-with-it.md#working-with-data).

## What it does

Each item links to its own section. The one-liners are here so you can tell
from this page whether the thing you want exists.

**[Capabilities](docs/capabilities.md)**: what it reaches for, and what it
writes down.

- [**Instructions**](docs/capabilities.md#instructions): reads the `AGENTS.md`
  and `CLAUDE.md` you already have, so you don't need a seventh copy.
- [**Skills**](docs/capabilities.md#skills): works a procedure out once, writes
  it down, and asks before keeping it. Nothing is saved behind your back.
- [**Memory**](docs/capabilities.md#memory): writes down what you'd otherwise
  have to tell the next conversation in this workspace again. You can read
  every note and forget any of them.
- [**Sub-agents**](docs/capabilities.md#sub-agents): delegates to a scoped
  context with its own tools, so a search that would fill the window happens
  somewhere else.
- [**Slash commands**](docs/capabilities.md#slash-commands): one namespace for
  skills, sub-agents, and built-ins.

**[Permission, and undo](docs/safety.md)**: what it asks before acting, and how
you put things back afterwards.

- [**Permissions**](docs/safety.md#permissions): every write shows up as a diff
  before you approve it. You can remember a decision for this project or for
  everywhere.
- [**Hooks**](docs/configuration.md#hooks): your own programs, run at fixed
  points in a turn. A hook can refuse a call but never approve one.
- [**Trusting a workspace**](docs/safety.md#trusting-a-workspace): a cloned
  repo's own config doesn't configure your agent until you say so, and you're
  only asked when the folder actually holds something. Next to the count, it
  names what's *in* those files, including the parts you can't see by looking:
  a byte that renders as nothing, a comment the renderer hides, a run of base64
  in a file that should hold prose.
- [**Running commands**](docs/safety.md#running-commands): a real PTY on each
  platform, so a program that checks `isatty` behaves the way it does in your
  terminal.
- [**Commands that keep running**](docs/safety.md#commands-that-keep-running):
  a cold build, a whole test suite, or a dev server gets started and left alone
  instead of waited on. What it changes is still undoable, from the files as
  they were before it ran.
- [**Rewinding a turn**](docs/safety.md#rewinding-a-turn): every file a turn
  touches is recorded first, so you can undo any turn.
- [**Keeping a turn**](docs/safety.md#keeping-a-turn): read a turn back as a
  diff and commit it on its own.
- [**Reviewing a turn**](docs/safety.md#reading-it-back-to-somebody-who-did-not-write-it):
  hand that diff to an agent that has seen none of the conversation. An agent
  checking its own turn looks for the mistake with the same reasoning that
  made it. This one has never seen the code. It can read but not write, and it
  can't know what you asked for. That's the point, and it's also the cost.

**[Working with it](docs/working-with-it.md)**: what a turn looks like in use.

- [**Sessions**](docs/working-with-it.md#sessions): transcripts on disk, per
  workspace, replayable.
- [**Finding a conversation**](docs/working-with-it.md#finding-a-conversation):
  ⌘K opens one box over the window. It finds every panel and action by name,
  conversations by title, and transcripts by what was said in them. Picking a
  hit opens the conversation *at* that hit.
- [**Planning a long task**](docs/working-with-it.md#planning-a-long-task): a
  plan it pins and keeps current, not one it announces once and forgets.
- [**Showing it a picture**](docs/working-with-it.md#showing-it-a-picture):
  paste a screenshot in.
- [**Finding code by what it does**](docs/working-with-it.md#finding-code-by-what-it-does):
  local semantic search, no external service.
- [**When a turn stops**](docs/working-with-it.md#when-a-turn-stops) and
  [**the context window**](docs/working-with-it.md#the-context-window): what
  ends a turn, and what it costs. The meter above the composer shows how much.
  Click it to see what it went *on*: a row per tool, the calls that exactly
  repeated an earlier one, and what every request pays before the conversation
  even starts.
- [**Tables, charts, diagrams, and questions**](docs/working-with-it.md#tables-charts-diagrams-and-questions):
  results you're meant to *look* at sit on their own beside the text,
  including sequence and flow diagrams. When a decision is really yours, it
  asks and waits, and you can skip any question.
- [**Code is colored, and so is a diff**](docs/working-with-it.md#code-is-colored-and-so-is-a-diff):
  a fenced block is colored by its fence, a diff by the file's extension. One
  palette covers both, and the query box too. Where one line was rewritten into
  another, the characters that actually changed are marked inside it.
- [**Motion that says what it is doing**](docs/working-with-it.md#motion):
  while a turn runs, it draws a waveform shaped by the kind of work in
  progress. A running row shows a scan, a write gutter, or an indeterminate
  hairline. A spinner only tells you a turn is alive. This tells you what it's
  busy with.
- [**Working with data**](docs/working-with-it.md#working-with-data): a CSV with
  a million rows in it isn't a file to read. Taurus loads it as a table,
  describes every column from the whole file instead of a sample, answers
  questions about it in SQL, and puts the rows on a pane of their own. A
  transformation worth keeping becomes a
  [**recipe**](docs/working-with-it.md#recipes): a chain of SQL steps committed
  with your code. You can re-run it on next month's export, and it reports
  what each step did to the row count.
- [**A query box that knows your columns**](docs/working-with-it.md#writing-the-query):
  SQL is colored as you type, and completion draws on the real schema of
  every loaded file, not a keyword list. A column two files share is marked
  `joins`, so you find the key to join on while you're writing the join.

![The Data pane: a profile of a 400,000-row file, with the missing values
marked](docs/screenshots/data.png)

The pane doesn't exist until a workspace has loaded something. It takes the
center column beside the conversation instead of covering it. The box you type
in never moves, because asking is still how anything gets here.

It works both ways:

- A message sent from the pane carries what's on screen, so "which category
  refunds most?" has something to refer to.
- A query the model ran leaves a card in the transcript with **Run in Query**
  on it. That asks the same question again in the pane, at full width.
- A failed query or a failed recipe run offers itself back to Taurus with the
  error already in the message.

**[The canvas](docs/capabilities.md#the-canvas)**: a file open beside the
conversation, not in place of it.

Ask to see a file and it opens in an editor to the right of the transcript.
"Show me where the retry logic is" opens the file with that passage selected.
The model passes the lines it means, so it can point instead of quoting.

![A Markdown file open beside the conversation that asked for it, with the
lines the model pointed at selected](docs/screenshots/canvas.png)

Select a passage and **Ask about this** starts a sentence in the message box.
The selection itself goes with that message: which file, which lines, and what
they said. So "tighten this" is a complete question to the model, the same way
it is to you looking at the screen. A chip above the box shows that while you
type, because context you can't see is behavior you can't explain.

You can type in it, and it saves a second after you stop. That's not just a
convenience. The whole point of the canvas is that you and Taurus are looking
at the same file. An unsaved buffer breaks that without telling you: you'd ask
about the paragraph on screen and get an answer about the one on disk.

Taurus writes files too, and neither of you waits for the other. If it changes
the file you have open and you haven't typed anything, the editor takes the
new version and tints what moved. If you *have* typed something, nothing gets
replaced. Both versions are kept, and you choose which one survives.

![Taurus changed the file while it was being typed in, so both versions are kept
and neither is chosen](docs/screenshots/canvas-conflict.png)

A save never overwrites a version it hasn't seen. The editor keeps a
fingerprint of the file as it read it, and a save that no longer matches is
refused. The race is closed at the write, not papered over.

**[Notes](docs/working-with-it.md#notes)**: somewhere to write things down, in
the repo or in your home directory.

A **Notes** tab sits beside the conversation, holding Markdown files you write
yourself:

- A *project* note lives in `.taurus/notes/` and gets committed with the repo,
  so a design note reaches whoever clones it.
- A *global* note lives in `~/.taurus/notes/` and follows you between
  projects.

There's no frontmatter and no database. The name in the list is the filename,
and the file is exactly what you typed. Type `/` at the start of a line for a
heading, a list, a table, a diagram to start from, or one of your sketches,
and a list carries on when you press Enter. Write and Read can sit side by
side, a task ticks from the rendered note, and a link to another note opens
it.

![The notes pane: both notebooks in the list, one note open in an editor that
wraps](docs/screenshots/notes.png)

A ```` ```mermaid ```` block draws as a diagram. The same two engines that draw
`show_flow` and `show_sequence` draw it, not the Mermaid library. So it uses
the app's own palette, needs no network, and comes out the same every time.
Those cards can already *write* Mermaid, and this is the other half of the
round trip: copy a diagram out of a card, paste it into a note, and you get the
same picture back.

![The same note in Read, with its Mermaid fence drawn as a
diagram](docs/screenshots/notes-diagram.png)

When it can't draw something, it says so. A `gantt` or an `erDiagram` shows as
source with a sentence explaining why, not as a broken picture or an empty
box. A diagram it *did* draw tells you what it left out. Notes save themselves
and never overwrite a version they haven't seen. That's the canvas's rule, and
the canvas's code.

A **sketch** is a drawing kept beside your notes, made in
[Excalidraw](https://excalidraw.com): freehand, shapes, arrows, handwritten
text. It's a `.excalidraw` file in the same notebook, saved the same way. A
note shows one with a line of Markdown:
`![How a sign-in goes](<Auth flow.excalidraw>)`. **Copy embed** on the sketch
gives you that line. A sketch reopens at the zoom and the spot you left it.

![A sketch open in the editor beside the list of notes](docs/screenshots/sketch.png)

![The sketch drawn into a note that embeds it](docs/screenshots/notes-sketch.png)

Ask about a note and the model reads it with `read_note`, by notebook and name
instead of by path. A global note is outside the workspace, where `read_file`
won't go. It can write one with `write_note`, which asks first and shows the
diff. It can also put one in front of you with `open_note`, which draws a card
you open yourself. It can't see a sketch, but it's told the words written on
any sketch a note embeds.

**[Configuration](docs/configuration.md)**: providers, keys, MCP servers, and
web search.

- Local Ollama, anything OpenAI-compatible, Anthropic, or Google Gemini.
- Keys live in the OS keychain or an env var, never in a config file.
- Everything the Settings drawer writes is a plain file the CLI reads too.
- [**MCP servers**](docs/configuration.md#mcp-servers): add and test them in
  the app, in the same `mcpServers` format Claude Desktop uses.
- [**Themes**](docs/configuration.md#themes): fourteen colors, three
  typefaces, a wordmark, and a corner radius, in a file you can commit. A
  workspace can carry its own, so a repo can brand the app for everyone who
  opens it.

![The MCP panel](docs/screenshots/mcp.png)

![Settings, Appearance](docs/screenshots/appearance.png)

**[Development](docs/development.md)**: tests, the live checks, the app icon,
regenerating these screenshots, and cutting a release.

**[Known gaps](docs/known-gaps.md)**: what it doesn't do, written down so you
can read about it before you run into it.

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
  taurus-data/              Reading, profiling, and transforming tabular files, behind one engine trait
  taurus-core/              Session state, the agent loop, sub-agents
  taurus-host/              Config, system prompt, registry assembly
  taurus-cli/               The `taurus` binary
src-tauri/                  Windows and IPC — no agent logic
src/                        React UI
```

One rule holds the design together: **a frontend contains no agent logic.**
Everything a session *is* (config files, system prompt, tool registry, skill
library) lives in `taurus-host`, and `Host::build_agent` is the only place
they come together. The desktop app and the CLI differ only in how they talk to
you: how they ask for permission, and where a skill proposal goes. Both of
those are traits.

That's also why the agent loop can be tested against a scripted provider, and
why the [live checks](docs/development.md#live-checks) run without a GUI.

### Provider-agnostic, concretely

The normalized types use Anthropic-style content blocks, not OpenAI's
`tool_calls` shape. Blocks are the superset. A single assistant turn with
interleaved text, reasoning, and several tool calls survives the round trip.
Going the other direction, it doesn't.

Three things show the abstraction works, instead of just claiming it does:

- **No adapter after the first needed a change to `taurus-core`.** The OpenAI
  one differs in transport (SSE vs NDJSON) and in tool-call encoding
  (arguments as a string assembled across frames vs a whole object). Gemini
  differs in what a conversation even *is*:
  - the assistant is called `model`
  - tool calls carry no ids at all, so results pair with calls by name
  - schemas are an OpenAPI subset, not JSON Schema
  - every streamed chunk is a whole response object, not a delta envelope

  None of that reached the core.
- **Models without tool support still call tools.** `gemma3` doesn't accept a
  `tools` parameter at all. Taurus detects that from Ollama's capability probe
  and switches to prompted tool calling. It parses `<tool_call>` blocks out of
  the text stream into the exact same events a native adapter emits, and
  `taurus-core` can't tell which path a turn took.
- **A backend that can describe itself gets asked, not configured.** Ollama
  and Anthropic both answer questions about their own models. So neither needs
  a `context_length` in `providers.json`, and neither can be told the wrong
  one.

## License

MIT. See [LICENSE](LICENSE).
