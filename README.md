# Taurus AI Shell

An agent harness that runs against any model provider — local Ollama, anything
OpenAI-compatible, Anthropic, or Google Gemini. Rust underneath, with two
frontends over one shared core: a Tauri v2 desktop app and a `taurus` CLI.
macOS, Windows, and Linux from one codebase.

It reads and edits files in a workspace, runs commands, searches the web,
connects to MCP servers, delegates to sub-agents — and writes down procedures it
works out as reusable **skills**, which you approve before they are kept. It
reads the `AGENTS.md` and `CLAUDE.md` you already have rather than asking for a
seventh copy. Every file it edits is recorded first, so any turn can be
**rewound**, and every write is shown as a **diff** before you approve it.

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
why the examples below run without a GUI.

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

### Instructions

A skill is a procedure the model loads when it needs one. Instructions are the
opposite: a short standing brief that applies to every turn — this project's
conventions, how you want work done, what not to touch. Taurus reads the files
you already have rather than asking for a seventh copy, on the same rule the
skill library follows. Six locations, lowest precedence first:

```
~/.agents/AGENTS.md         <workspace>/AGENTS.md
~/.claude/CLAUDE.md         <workspace>/CLAUDE.md
~/.taurus/TAURUS.md         <workspace>/.taurus/TAURUS.md
```

The project files sit at the repository root rather than inside a dotdir,
because that is where they actually live — a repo's brief is `AGENTS.md` beside
the README, and looking anywhere else would find nothing in the projects this
exists for.

**They accumulate rather than shadow**, which is the one deliberate difference
from skills. Two skills named `deploy` are rival answers to one question, so the
project's wins. "I prefer terse commit messages" and "this repo pins its
toolchain" are both true at once, and dropping either because the other exists
would be a silent loss. Each file is labelled in the prompt with where it came
from, so a model can tell a personal preference from a project requirement — and
the section says the project's win where they disagree.

Two files with identical bytes are read once. `CLAUDE.md` symlinked to
`AGENTS.md` is the common shape, and a rule the model is told twice is a rule it
weights twice.

**`@path` imports are resolved**, one level deep. Claude Code's format lets a
file be a list of pointers, and real ones are: a global `CLAUDE.md` whose entire
content is `@RTK.md` is a file Taurus would otherwise read as a single
meaningless line. A line qualifies only when the whole of it is `@` followed by
a path, so `Ask @alice before releasing` is prose and stays prose. An import of
a missing file is reported rather than passed through — a pointer at nothing
tells the model less than nothing.

A file longer than 12 KB is cut on a line boundary and says so, in the prompt
and in the Skills drawer. These bytes are paid on every request of every turn,
so a checked-in handbook would otherwise spend an 8k model's whole context
before it read a line of code.

The brief lands directly after the harness's own rules and before the skill
catalog. That ordering is the design: a brief saying "ask before touching the
database" argues with "keep going until the task is done", and a small model
settles a contradiction by recency — so the brief comes second, where it wins.

### Skills

A skill is a `SKILL.md` with YAML frontmatter plus optional bundled scripts, in
the format defined by the [Agent Skills specification](https://agentskills.io/specification).
Taurus reads the shared locations as well as its own, so a skill installed by
another client works here without being copied. Six directories, lowest
precedence first:

```
~/.agents/skills            <workspace>/.agents/skills
~/.claude/skills            <workspace>/.claude/skills
~/.taurus/skills            <workspace>/.taurus/skills
```

A project skill shadows a personal one of the same name, and within either
tier a `.taurus` skill shadows a borrowed one — so you can override a skill you
did not write without editing it. The drawer tags each row with where it came
from, and a shadowed skill is logged rather than silently dropped.

Only one line per skill enters the system prompt: its `when_to_use` when it has
one, and a condensed `description` otherwise, which is every skill written for
another client. The procedure itself loads on demand via the `load_skill` tool.
That is what makes a fifty-skill library affordable on a model with an 8k
context window.

`when_to_use` is a Taurus field and optional. It is worth writing for skills
you keep here: the specification's `description` does two jobs at once — what
the skill does and when to use it — and 200 characters aimed squarely at the
decision beats 1024 aimed at a catalog listing when the whole context is 8k.

Loading is lenient, because a skill you already have installed is more useful
read than refused. A name in the wrong case, a name that disagrees with its
directory, a value with an unquoted colon — each is repaired or tolerated,
reported on the skill's row in the drawer and by `taurus skills check`, and the
skill loads. Only an empty description or YAML no quoting can rescue stops it.
Skills Taurus writes itself are held to the strict rules: a proposal that would
only load by leniency is rejected rather than written.

Any skill can also be run directly as a slash command — `/speckit-specify add a
dark mode toggle` — in the app and in `taurus run` alike. The harness resolves
the name against the library, fills the skill's `$ARGUMENTS` placeholder with
the rest of the line, and hands the model the procedure instead of the command;
a skill with no placeholder gets the text appended under a heading rather than
losing it. The composer completes names as you type `/`, and a name nothing
matches is reported to you rather than sent, with the skill it resembles.

Two frontmatter flags decide the ways in. `disable-model-invocation: true`
keeps a skill out of the prompt catalog while leaving it runnable by name — for
procedures that should run when a person asks and not before. `user-invocable:
false` does the reverse. Both are optional and both default to available.

Ordinary text that begins with a slash is never treated as a command:
`/usr/bin/env is portable` is sent as written. A command has to name a skill,
start with a letter, and be followed by a space or nothing.

A skill's `scripts/`, `references/`, and `assets/` are listed when the skill is
opened and read only if the procedure calls for one — the third tier of
progressive disclosure. Scripts left in `scripts/` without being declared in
the frontmatter are picked up by extension, so a skill written for another
client is runnable rather than merely readable. For the same reason, read-only
tools may reach into the directories of loaded skills; writes stay inside the
workspace.

The agent proposes new skills through `propose_skill`. Every proposal is
validated (kebab-case name, non-empty trigger under 200 characters, no
near-duplicate of an existing skill, no destructive script patterns) before it
reaches a review card, and nothing touches disk until you approve it. Approving
reloads the catalog, so a skill is usable in the session that wrote it.

Scripts declare a logical interpreter (`python3`, `node`, `bash`, …) which is
resolved per platform at load time. When it cannot be found, the skill is
marked degraded and the model is told to follow the written steps instead — a
Python-dependent skill does not hard-fail a Windows machine.

### Sub-agents

A turn can hand a self-contained job to a sub-agent: its own conversation, its
own context window, and a narrower set of tools. The parent sees only the
child's conclusion, so a search that reads thirty files costs it one paragraph.

Two ship with the harness. `explorer` searches and reads and cannot modify
anything; `worker` carries out one well-specified change with the tools the main
agent has. The model reaches either through `spawn_subagent`.

You can add your own. An agent is a markdown file in `~/.taurus/agents` or
`<workspace>/.taurus/agents` — the file name is the agent's name, and the body
below the frontmatter is its system prompt:

```markdown
---
name: reviewer
description: Reviews a diff for correctness bugs. Use after a change is written.
tools: [read_file, grep, glob]
max_iterations: 20
model: qwen3:32b        # optional; defaults to the session's model
provider: ollama        # optional; defaults to the session's provider
---

You are a review sub-agent. Read the diff you were given and report only
defects you can point at a specific line for. You cannot ask questions.
Be brief; the agent that called you sees only your reply.
```

A project agent shadows a personal one of the same name, and either shadows a
built-in — so a `explorer.md` of your own replaces the shipped explorer rather
than sitting beside it. The drawer says on the row when that has happened.

`tools:` is **enforced**, unlike a skill's `allowed_tools`, which is advisory:
it is exactly the set the child is offered. Leave the key out to inherit
everything the main agent has. It narrows what the agent is *offered* and never
what it is *permitted* — every call the child makes still meets the same
permission gate as the parent's. If every tool an agent names turns out to be
unavailable here, the agent is refused rather than run unscoped, because an
empty list would otherwise mean "everything".

`model:` runs one agent somewhere else — a bigger model for review, a smaller
one for search. Naming a provider that is not configured on this machine
degrades the agent rather than failing the load: it runs on the session's model,
and both the drawer and `taurus agents check` say so. A repo can ship an agent
naming a cloud model without breaking for a contributor who runs Ollama only.

A sub-agent cannot delegate further. Its registry has no `spawn_subagent` in it,
so the depth cap is structural rather than a counter the model could talk past.

```bash
taurus agents list    # the roster, what each is scoped to, what it costs
taurus agents check   # non-zero if an agent will not load or cannot run as written
```

Authoring is a text editor, as it is for skills. The drawer's **New agent…**
writes a starter file with every key documented in place and opens it, and it
rescans on open, so editing a file and reopening shows what is actually on disk.

The agent can also write one for you. `propose_agent` is the twin of
`propose_skill` and gated by its own setting — a skill is a procedure the model
follows, an agent is a worker it hands a task to, and wanting one is no reason
to want the other. A proposal is validated before it reaches a review card:
kebab-case name, a description under 200 characters, a system prompt long
enough to be worth a file, no near-duplicate of an agent already on the roster,
and no tool this session does not have. Nothing touches disk until you approve
it, and the card is editable — so it is validated again on the way out, because
a hand-edited name or tool list has never been checked.

What keeps that a bounded risk is that a proposed agent cannot reach past the
session that wrote it. `tools:` only ever narrows; a name outside the session's
registry is refused rather than saved and degraded; every call the child makes
still meets the parent's permission gate; and the child has no `spawn_subagent`,
so it cannot propose or spawn further agents. `model:` and `provider:` are not
proposable at all — which model a delegate runs on is a cost decision on a
provider you pay for, and it is the one field with no bearing on what the agent
can do. An approved agent inherits the session's model; change it by editing
the file, where the decision is yours and visible.

Approving rescans the roster rather than reloading everything, so saving an
agent does not restart every MCP server. It is not usable in the turn that
proposed it — a turn's roster is frozen when it starts — and the tool result
says so, rather than letting the model spend a round trip finding out.

### Permissions

Read-only tools inside the workspace run unattended. Writes, command execution,
and network access prompt with the exact call. Shell approvals are keyed by the
leading command word, so approving `git` does not also approve `rm`. A call that
names a URL is keyed the same way by that URL's host: approving `fetch_url` for
`docs.rs` is a decision about a site, not a standing grant to reach anywhere.

**A write is shown as a diff.** `Write src/widget.rs (2140 bytes)` says a file is
about to be replaced and nothing about what with. For a new file that is the
whole story; for an overwrite it is the least informative moment in the product,
since the bytes being destroyed are on disk and the bytes replacing them are in
the tool call. So they are diffed, in the desktop dialog and on the terminal —
the pre-image read is the one the checkpoint log takes a moment later, so this
costs a read that was going to happen anyway.

![The permission dialog showing a diff of the change a write would
make](docs/screenshots/permission-diff.png)

The diff `edit_file` shows is computed by running the replacement the call will
run, through the same function. A dialog that shows one change while the tool
makes another is worse than showing none, because it is what the user believed
when they approved it. Both are capped at 160 lines and say how many they left
out — a wall of lines in a modal gets approved unread, which is the failure this
prevents arrived at from the other side. A write that would change nothing says
so rather than showing an empty frame; that is usually a model looping, and it
is a decision worth not making. A file that is not text produces no diff at all,
because a diff of replacement characters is the same non-answer the byte count
already gave.

An "allow always" decision persists into one of two layers, and both are
consulted:

| | Persists to | Asked again in a new workspace |
| --- | --- | --- |
| **Always here** | `<workspace>/.taurus/permissions.json` | Yes |
| **Always everywhere** | `~/.taurus/permissions.json` | No |

**Everywhere is not offered for running commands.** A workspace grant for `git`
is scoped to a project you have already decided to trust; the same grant
globally applies in every repository you ever open, including one you just
cloned to look at. That is the decision most worth making per project. The
restriction governs what Taurus will *create* — a `run_command:*` rule written
into the global file by hand is still honored, because editing that file is an
explicit act and silently ignoring it would be its own surprise.

Every path argument is canonicalized and checked against the workspace root,
which closes `../` traversal and symlink escapes alike.

**With no terminal** — a pipe, a git hook, CI — there is nobody to prompt, and
both obvious defaults are wrong: allowing everything hands an unattended model
the machine, denying everything is useless without saying why. So the CLI takes
an explicit policy and names the flag that would have permitted what it
refused:

```
$ taurus run "summarize the readme into SUMMARY.md" < /dev/null
  refused (write): Write SUMMARY.md (214 bytes)
    no terminal to ask; re-run with --allow write_file to permit it

$ taurus run --allow write_file "summarize the readme into SUMMARY.md"
```

`--allow-command git` grants a shell program by its leading word, the same unit
the interactive "allow always" uses. `--dangerously-allow-all` exists for
throwaway or already-sandboxed environments.

Skills are never saved unattended. If the agent proposes one during a piped
run, the CLI reports it and discards it rather than writing something nobody
reviewed.

### Running commands

Commands run with three pipes and no stdin by default, which is right for almost
everything an agent runs: a model cannot answer a `[y/N]` prompt, so a command
that waits for one must fail on the timeout rather than hang the session.

The exception is the program that asks whether it is talking to a terminal. Told
no, `git` pages and colors nothing, `npm create` declines to scaffold, and
anything built on a full-screen prompt library fails at startup — behavior a
person would never see and cannot easily explain. Those are not exotic commands.
They are the ones somebody would reach for.

So `run_command` takes two more arguments:

- **`pty: true`** runs the command under a real pseudo-terminal — `forkpty` on
  Unix, ConPTY on Windows, from the one codebase. The program believes in a
  terminal because there is one.
- **`stdin`** hands it the keystrokes up front. A pty is a different thing from
  interactivity: under one, a program that wants an answer still waits for it,
  so the two arrive together. This is what turns "behaves correctly" into
  "completes", and it works on the piped path too.

A terminal has one stream, so under a pty stdout and stderr are interleaved and
the `[stderr]` split the model reads elsewhere is not recoverable. That is a
property of the thing rather than of this implementation. Terminal control
sequences are stripped before the output reaches the model: a `cargo build`
under a pty is more escape bytes than text, and the model pays tokens for every
one of them. Bare carriage returns go with them, so a progress bar does not
produce a transcript that scrolls over itself.

The timeout still holds, and has to: under a pty an interactive program waits
rather than hitting end-of-file, so a ceiling that did not fire would hang a
session for good. It kills the child rather than abandoning it — a blocking read
cannot be cancelled, and a worker parked on a child that will never exit would
otherwise outlive the session that started it.

### Sessions

Every conversation is written as it happens, so closing the app or the terminal
does not end it:

```bash
taurus sessions                       # this workspace's, newest first
taurus repl --resume                  # pick up the most recent one
taurus run --resume <ID> "and now…"   # continue a named one
```

The desktop app reopens its last conversation for the workspace on launch, and
its left rail lists the rest — today's, then everything earlier — so switching
between them is one click rather than a drawer.

Transcripts live in `~/.taurus/sessions/<workspace>/<id>.jsonl`, in the global
config home rather than in the project. They hold file contents, command
output, and MCP responses; kept inside the workspace they would be committed by
accident. Keying the directory by workspace gives back the only thing that
location cost — "show me this project's sessions" — without putting any of it in
the repository.

The file is append-only, one JSON object per line: a header, then each message
as it is produced. Nothing is rewritten, so a crash costs the turn in flight
rather than the conversation, and a half-written final line is dropped on load
instead of poisoning the file. There is no index — everything a listing shows is
in each transcript's own opening lines, and an index is a second copy of the
truth that can disagree with it.

### Rewinding a turn

A transcript remembers that the model called `edit_file`. It does not remember
the bytes that were there first — so a model that rewrites the wrong file, or
gets an edit subtly wrong across a dozen call sites, has destroyed work nothing
else in the harness can give back.

So the bytes are kept. Before a tool changes a file, its current contents go
into an append-only log beside the transcript, and any turn can be undone:

```bash
taurus rewind                        # turns that changed files, newest first
taurus rewind --to last --dry-run    # exactly what undoing the last one does
taurus rewind --to 3                 # back to just before turn 3
```

The desktop app's **Changes** drawer is the same thing with a button.

Rewinding to turn *N* undoes every turn from *N* onward, not only that one: the
log records what a file held before a turn, and restoring one turn while
leaving a later one in place would produce a tree that never existed. Where two
turns touched the same file, the oldest pre-image wins, because that is the one
that predates all of them.

Both frontends show the plan before they write, and neither will do it
unattended — a rewind discards whatever is in those files *now*, including
edits you made by hand since. Piped, it names the flag that would have allowed
it, the same way a refused tool call does:

```
$ taurus rewind --to last < /dev/null
  reverted  src/widget.rs
  deleted   src/widget_test.rs
taurus: no terminal to confirm on; re-run with --yes to rewind, or --dry-run to
        see the plan
```

A file that was not text when it was recorded is reported as `skipped` rather
than silently left as the model made it, and `taurus rewind` exits non-zero
when anything could not be put back.

#### Commands are covered too

A tool that can name what it will change declares it, and the log reads the
file just before the call. `run_command` can name nothing — a command line does
not say which files it will rewrite, and a guess would be worse than no answer.

So it is not asked. The workspace is indexed before the command runs and walked
again when it finishes, and the difference is the answer: anything whose length
or modification time moved, appeared, or vanished is a change, and the contents
held from the first pass become its pre-image. A rewind then treats it exactly
like an `edit_file`. `sed -i` across a dozen files, a `rm` that took the wrong
directory, a script the model wrote and ran — all of it comes back, and all of
it appears in the changed-file count and the **Changes** drawer, which is where
you look to decide whether you want it back.

This runs whether the command succeeded, failed, timed out, or was canceled: a
command killed halfway through has still written whatever it got as far as
writing, and that is exactly the turn undo is wanted for.

What it walks is the workspace minus `.git` and minus `.taurus`, and it draws
one more line: a directory `.gitignore` excludes is not entered, but a file it
excludes by name is still covered. Indexing `target/` and `node_modules/` would
cost gigabytes on every command, and a rewind that deleted build output would be
a worse surprise than one that leaves it alone — while the file an ignore rule
usually names is `.env`, and a command that clobbers *that* is exactly the one
you want back.

The split falls out of the walk rather than being a list of blessed names.
Every directory the walk enters is also read flat, which turns up the entries
the walk would skip; a directory it never enters is never read either. So `.env`
beside a `Cargo.toml` is covered and `target/` beside it is not. On this
repository the two passes cost about 21 ms and 8 ms.

Because `.env` is held, the checkpoint log holds it too, so those logs are
readable by their owner and nobody else. A file kept out of version control on
purpose should not become world-readable by being made recoverable.

**A command that moved git says so.** `.git` is not swept and not restored, so
undoing a turn that ran `git checkout` or `git reset --hard` puts the files back
and leaves `HEAD` where the command left it — a tree matching neither commit.
Restoring that properly would mean snapshotting the object store, which is its
own feature; noticing costs two small reads, so the sweep reads `.git/HEAD` and
the branch it names before and after each command. When both moved and files
were recorded, the result carries the reason it is not the whole story:

```
[taurus] This command moved git's own state as well. A rewind puts the files
back but leaves HEAD and the index where the command left them, so the result
would match neither commit; `git reflog` is the way back to where HEAD was.
```

Only when files were recorded, which is what keeps it a warning rather than a
running commentary. A `git commit` that touched no working-tree file leaves
nothing to undo, so nothing looks undoable and there is nothing to correct.
`.git/index` is watched by neither: `git status` rewrites it to refresh its stat
cache, and a note on every turn that ran one would drown the turns that matter.

When a command *cannot* be covered — a workspace past 50,000 files, or one
whose ignore rules the command itself rewrote — the tool result says so in
plain words rather than letting the turn look undoable:

```
[taurus] This workspace holds more than 50000 files, too many to record a
command's changes against, so this one cannot be undone.
```

### When a turn stops

A turn runs until the model stops asking for tools. Three things end one early,
and each records its reason in the transcript so a resumed session finds an
explanation rather than a conversation that simply stops:

- **The iteration ceiling** — twenty-five model/tool round trips. A ceiling
  rather than a budget the model is shown, because one it could see is one it
  could argue with.
- **A stall** — the same tool call, with the same arguments, failing three
  times with nothing succeeding in between. The system prompt already tells the
  model not to retry a failed call unchanged; this is what makes that true
  rather than merely stated. Counted across rounds rather than consecutively,
  so a model alternating between two dead ends — A, B, A, B, A — is caught as
  readily as one insisting on a single call. Anything that succeeds clears the
  count, which is what keeps a model working through genuinely different
  candidates from tripping it: a model re-reading a file it is editing is
  working, not stuck.
- **A provider failure no retry could fix** — a rejected key, an unknown model,
  a response that would not parse.

A rate limit or a 5xx is not in that last group. Those are retried up to three
times with a doubling backoff, and the wait is reported rather than silent,
because a pause nobody explained is indistinguishable from a hang. Cancelling
during a backoff returns immediately instead of serving out the delay.

One case is deliberately never retried: a request that had already begun
streaming an answer. The user has read the first half, and a second attempt
would write it again.

#### One thing extends a turn

A turn that changed files and never ran anything afterwards is asked, once, to
check its own work before it is allowed to finish:

> You changed files and have not run anything since. Check that work now — run
> the project's tests, or build it, or run the thing you changed. If there is
> genuinely nothing to run against it, say so in one line and stop.

The system prompt says the same thing, and saying it there is not enough: a 9B
model edits a file and stops anyway. Asked at the moment it tries to finish, it
goes and runs the build. What counts as having checked is a command that ran
and changed nothing — the model asking the project a question and getting an
answer. A command that changed files as well is more work, not a check.

Once per turn, and phrased with a way out, so a documentation edit costs one
round trip rather than an argument. `verify_changes` in `AgentConfig` turns it
off. The checkpoint log is what it reads to know whether anything changed,
which means it is exactly as accurate as the log — a command that only touched
files inside an ignored directory reads as having changed nothing.

### The context window

A local 8k model runs out of room in a way a hosted 200k one does not, so the
budget is managed rather than hoped for. Three things do the work.

**Reads come back a window at a time.** `read_file` returns 2000 lines by
default and takes `offset` and `limit` for the rest. Line numbers stay absolute,
so a number from a windowed read still means what it says, and a partial answer
always says it is partial — a window that does not announce itself is
indistinguishable from a short file, and a model that thinks it read the whole
thing will act on what is missing.

**Old tool output shrinks before anything is summarized.** Tool results are most
of what a working session holds, and every byte is re-sent on each iteration of
the turn. When history crosses the compaction threshold, two cheap rules run
first, with no model call involved: a result whose call was later repeated with
the same input is dropped down to a pointer at the newer one, and results older
than the verbatim tail keep their first few lines and say what went. Only if
that does not get under budget is the older half summarized. The block itself
always stays either way — replacing its text keeps every tool call paired with a
result, which is the thing providers actually validate.

**Nothing is advertised that the prompt cannot explain.** Every tool's schema
goes out with every request — not once per session, once per iteration of every
turn — so it is the one part of the prompt that is pure overhead. Three things
keep it down. Schemas are slimmed on the way out: `$schema`, the Rust struct
name `schemars` leaves in `title`, `"default": null`, and integer-width formats
are dropped, which is about a quarter of the built-in schema bytes and applies
to MCP servers' schemas too. `propose_skill` and `propose_agent` are each only
registered when their own setting is on, matching the prompt section that
explains each — they are the largest schemas here, and offering one while saying
nothing about it was paying for a tool the model had no reason to call. And
anything a project does not want can be named in `settings.json`:

```json
{ "disabled_tools": ["fetch_url", "mcp__some-server__rarely_used"] }
```

A disabled tool is not registered at all, so skills and sub-agents cannot reach
it either — a tool hidden from the model but still callable would be a
permission gap wearing a token-saving costume. A name matching nothing is
reported rather than ignored, because a typo otherwise looks exactly like a tool
that is quietly still on.

Four tools a turn adds for itself can be named here too, though `taurus tools`
does not list them: it prints the set a *sub-agent* could be scoped to, and
these four are exactly the ones a sub-agent never gets. They are
`spawn_subagent`, which is the delegation depth cap, and `show_table`,
`show_chart`, and `ask_user`, which address the person watching this
conversation.

**Where it went is a question you can ask.**

```bash
taurus usage            # this workspace's most recent session, by tool
taurus usage --all      # every session in this workspace
```

```
Turns              1
Messages           16
Billed by provider 20,440 in / 496 out
Transcript holds   ~1,407 tokens

Tool                    calls    ~tokens   share
read_file                   7      1,144    100%

Sent again with every request  ~1,982 tokens
  system prompt                 430
  10 tool schemas             1,552

Heaviest tool schemas
  propose_skill                 456
  read_file                     166
  run_command                   160
  edit_file                     156
  run_skill_script              151
  5 more                        463
```

The gap between the first two figures is the point: a transcript holding 1,407
tokens billed 20,440, and the bottom half is where the difference went — ~1,982
tokens of fixed overhead on each of seven requests. Per-tool numbers are
estimates, since a provider reports one total per request and never says which
part of the prompt was whose, but they use the same arithmetic that drives
compaction, so the report and the trigger cannot disagree. They are read back
out of the transcript rather than tracked beside it, for the reason the
transcript format already gives: a second copy of the truth can disagree with
it.

### Output formatting

Models answer in markdown, so both frontends render it.

The app parses markdown progressively as tokens arrive, tolerating the
half-finished constructs that streaming produces — an unclosed `**`, a code
fence with no terminator yet. Raw HTML is never rendered: model output is not
trusted markup, so `rehype-raw` is deliberately absent and any tags arrive
escaped as visible text. Links open in your browser rather than navigating the
webview away from the app.

The CLI applies ANSI attributes line by line: headings bold, `•` for bullets,
colored inline code, dimmed fenced blocks. With color off — piped, redirected,
or `NO_COLOR` — every line passes through byte for byte, so `taurus run >
out.md` still produces valid markdown.

The trade-off is that CLI prose appears a line at a time rather than a token at
a time, since a line has to be complete before it can be styled. The
alternative — redrawing the current line with cursor escapes — corrupts output
as soon as a line wraps.

### Tables, charts, and questions

Three tools address the person watching rather than the machine. They change
nothing, need no permission, and their result to the model is only a
confirmation — what matters is what they put on screen.

| Tool | Draws | Reach for it when |
| --- | --- | --- |
| `show_table` | A sortable table, copyable as CSV | Several rows of comparable facts, and the comparison is the point |
| `show_chart` | A bar chart, with a tab per series | The shape of a series is the answer — where the spike is, whether a number is climbing |
| `ask_user` | A question card, and waits for it | A decision that is genuinely yours and would change what gets built |

Each is drawn from the call's own input, unchanged. That identity is what lets
a reopened conversation redraw the table rather than show a row saying one was
drawn once: a transcript records the model's messages and nothing about how
they were rendered, so a view that *is* its input survives a restart and a
derived one would not. A call the harness refuses draws nothing at all — the
view goes out before the tool runs, so it is withdrawn again if the tool then
rejects the arguments, live and on reload alike.

`ask_user` is the only one that blocks. The call parks until the card is
answered, exactly as a permission prompt does, and every question can be
skipped — "You decide" answers all of them at once and sends. This is the one
exception to the system prompt's instruction to keep going without stopping,
and the prompt says so beside the rule it breaks, because a small local model
given both and no reconciliation will pick the wrong one. It is not available
to sub-agents: a delegate has no user watching it, so `ask_user`, `show_table`,
and `show_chart` are all registered per turn alongside `spawn_subagent` rather
than in the shared registry the children inherit.

On the CLI, a table and a chart print in full to stdout in place of the usual
one-line "called a tool" annotation, so `taurus run > out.txt` keeps them.
Charts are drawn horizontally there — vertical bars need a height a scrollback
does not have — and every series prints, since a terminal has no tabs. A
question numbers its options and reads a line, with Enter alone to skip. Where
there is no terminal at all — a pipe, a git hook, CI — nothing hangs: the tool
comes back saying nobody was available, and the model is told to decide and say
which way it went.

## Configuration

The desktop app's **Settings** drawer edits providers, revokes permission
rules, and toggles skill and sub-agent synthesis. Everything it writes is a plain file under
`~/.taurus` that the CLI reads too, so the UI and a text editor are
interchangeable.

Every config file exists in two layers: the global `~/.taurus` and the
workspace's own `.taurus`. The workspace layer is read second and wins, the
same precedence skills use.

Settings edits the **global** layer, and says so when the current workspace
overrides one of the values on screen. That direction is deliberate: an editor
that saved the merged view back would write one project's overrides into the
file every other project reads.

| File | Global | Workspace |
| --- | --- | --- |
| `providers.json` | Backends, including the header a key is sent in. Never the key itself — that lives in the OS keychain or an env var. | Overrides and additions for this project. |
| `mcp.json` | MCP servers over stdio or HTTP, in the same format Claude Desktop uses. Header values and URLs may name env vars. Skills › **Edit mcp.json** opens it. | Extra servers, or `{"disabled": true}` to switch an inherited one off. |
| `search.json` | Web search backends and which one is active. Never the key itself — that lives in the OS keychain or an env var, as with providers. | A different backend for this project, or field overrides on an inherited one. |
| `settings.json` | Last workspace, the two synthesis toggles, theme, fallback model. | The provider and model this project was last worked in. |
| `skills/` | Skills available in every workspace. | Skills that travel with the project. |
| `permissions.json` | "Always everywhere" decisions. | "Always here" decisions. |
| `sessions/` | Transcripts, in a directory per workspace. | — |
| `checkpoints/` | Pre-images of changed files, keyed by workspace like sessions and for the same reason. | — |

### API keys

A key never goes in a config file. Type it into Settings and it goes to the OS
credential store — Keychain on macOS, Credential Manager on Windows, the Secret
Service on Linux — under the service `taurus` and the provider's id. Web-search
backends keep their keys in the same store, under `search:<id>`, since both ids
are yours to choose and a backend and a provider may well share a name. Or from
a terminal:

```bash
taurus key set openai        # prompts, input not echoed
taurus key set openai < key.txt
pass show openai | taurus key set openai
taurus key set brave --search   # the same, for a web-search backend
taurus key status            # where every key comes from, both kinds
taurus key clear openai
```

The key is read from stdin, never from an argument: a key on the command line
is visible to every process on the machine through `ps` and lands in the shell
history of whoever typed it.

**An environment variable wins over a stored key.** Exporting one is an
explicit act, usually in CI or a container where no keychain exists at all, and
a stored key silently beating it would make headless runs unpredictable. So
`api_key_env` remains what it was, and is now optional — name a variable and it
takes precedence, leave it unset and the stored key is used. `taurus key
status` and the Settings field both say which one is in effect, because
"I stored a key and it isn't being used" is otherwise a 401 that explains
nothing:

```
$ taurus key status
ollama               none
openai               $OPENAI_API_KEY  (a stored key is being overridden)
azure                keychain
```

Settings never displays a stored key, only where the key comes from. The field
is a place to type a new one, not to review the old one — a secret handed to
the webview lives in JavaScript memory and in whatever the DOM does with it,
and nothing on that screen needs the value.

Two things worth knowing. On macOS the keychain grants access per binary, so
the first time the `taurus` CLI reads a key stored by the desktop app (or the
reverse) the OS asks you to allow it; "Always Allow" makes it once. And on
Linux the Secret Service is a running D-Bus service, not a file — on a headless
box there may be none, in which case storing fails, `taurus key status` says
so, and environment variables are the whole story.

### MCP servers

`mcp.json` takes stdio and streamable-HTTP servers in the format Claude Desktop
and Claude Code use, so an existing `mcpServers` block pastes in unchanged:

```jsonc
{
  "mcpServers": {
    "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"] },
    "remote": {
      "url": "https://mcp.example.com/mcp",
      "headers": { "Authorization": "Bearer ${EXAMPLE_MCP_TOKEN}" }
    }
  }
}
```

`${VAR}` in a header value or a URL is read from the environment. That matters
because a remote server almost always needs a credential, and the workspace
layer of `mcp.json` is meant to be hand-written and version-controlled — a
literal token there is a token in the repository. It is the same bargain
`providers.json` already makes for API keys. A variable that is not set fails
the server with its own name in the message, rather than sending an empty
`Authorization` header and producing a 401 that looks like a bad token instead
of a missing one. A literal value still passes through untouched.

**The agent can draft an entry but never install one.** `draft_mcp_server`
takes a name and a command line and hands back a block to paste, the file it
belongs in, and what has to be filled in first — and that is all it does. It
writes nothing and starts nothing.

That asymmetry with skills and sub-agents is deliberate. Both of those are
reviewable: the artifact you approve is the text that will run. An MCP entry is
a pointer to code nobody in the loop has seen — the reviewable part of `npx -y
@scope/package` is a package name — and the program it names runs at every
launch, before any tool call, outside the permission engine. A review card
there would be asking for a decision with the information missing. So the model
does the part it is good at, which is knowing what the server is called and
which arguments it takes, and installing stays something you do in your editor
having read it.

Secrets are never carried through the draft. `env` and `headers` take variable
and header *names*; the block comes back with `<replace-me>` where each value
goes, and the model is told to explain what each one is rather than guess at it.
A key the model typed would live in the transcript, and in every copy of it, for
as long as the conversation is kept. The block is rendered through the same type
the loader reads, so what comes back is what will parse.

### Web search

Two tools, `web_search` and `fetch_url`, and **neither exists until you turn
one on.** Searching means sending your prompt to a third party, which is not
something to start doing because a program was installed.

In the app, that is **Settings › Search**: pick a backend, paste its key, done.
The key goes to the OS keychain, the same place provider keys go, and the tools
are registered the moment the backend resolves — no restart.

Everything it writes is `~/.taurus/search.json`, which the CLI reads too and
which a first run leaves behind with every backend spelled out and none of them
selected:

```jsonc
{
  "backend": "brave",           // ← unset means off
  "backends": {
    "brave":   { "kind": "brave",   "api_key_env": "BRAVE_API_KEY" },
    "tavily":  { "kind": "tavily",  "api_key_env": "TAVILY_API_KEY" },
    "searxng": { "kind": "searxng", "base_url": "http://localhost:8888" }
  }
}
```

`api_key_env` stays supported and **wins over a stored key** when both are set,
which is what makes CI and keychain-less machines work. Same precedence as
providers, because both resolve through the same code.

| `kind` | Needs | Notes |
| --- | --- | --- |
| `brave` | An API key | Free tier covers ordinary use. |
| `tavily` | An API key | Built for agents; returns page extracts rather than one-line snippets. |
| `searxng` | A `base_url` | No key and no account. Your instance has to enable the `json` format in its `settings.yml`, or it answers API requests with a 403. |

Backends layer field by field like providers, so a workspace can retarget one
setting — `{"backends": {"brave": {"base_url": "http://proxy.internal"}}}` — or
switch which one is active with a bare `{"backend": "searxng"}`, without
restating the rest. `base_url` may name an environment variable the same way
`mcp.json` does.

Both tools carry the `network` effect, so both prompt with what they would
send: `web_search` shows the query in full, and `fetch_url` shows the whole URL
rather than an abbreviation of it — which host a request goes to is the
decision being approved, and a shortened URL is exactly what hides it.

Redirects are followed only while the host stays the same. One approval is
worth exactly one host, and the default policy would spend it somewhere else:
approve a link shortener and the hop lands wherever it points, including on
your own network. A redirect that crosses hosts stops and reports its target,
which the model can then ask for on its own terms.

**`fetch_url` will not reach your own machine or network.** The host is
resolved and every address it answers with has to be public, so
`http://127.0.0.1:8080/admin` and `http://169.254.169.254/` — the cloud
metadata endpoint, which answers unauthenticated and with credentials — are
refused. That resolution happens inside the HTTP client `fetch_url` uses, so
the addresses the connection is given are the addresses that were checked;
there is no second lookup for a name to answer differently. The URL there is
chosen by a model that just read a web page, which
is what makes it different from a search backend or an MCP server: those are
addresses you wrote down, and they are never subject to this. If you want the
model reading a docs server you run locally, that is a deliberate act:

```jsonc
{ "allow_private_hosts": true }
```

Deliberately file-only, and not a checkbox in Settings. It is the one setting
here where the easy version of the mistake is expensive, and a config file is
a good place to make someone think for a moment.

The tools are registered together or not at all. A `fetch_url` with no way to
find a URL is only usable on links you paste, and search without fetch leaves
the model holding snippets it cannot follow. If the selected backend cannot run
— no key saved and none in the environment, a SearXNG entry with no URL —
nothing is registered and the reason is reported in Settings › Search, rather
than the model spending a turn discovering it has no credential. Picking a
backend and it still not running is a state the tab names explicitly, since a
selection alone is not the same as a working one.

### Anthropic and Google Gemini

Both are their own `kind` rather than a `base_url` pointed at a different host,
because neither is OpenAI-shaped. Anthropic reads the key from `x-api-key`, puts
the system prompt in a top-level field, and sends tool input as an object;
Gemini calls the assistant `model`, gives tool calls no ids at all, and takes an
OpenAPI subset where the others take JSON Schema.

```jsonc
[
  { "id": "anthropic", "kind": "anthropic", "base_url": "https://api.anthropic.com" },
  { "id": "gemini", "kind": "gemini", "base_url": "https://generativelanguage.googleapis.com" }
]
```

That is the whole configuration. Keys go in the OS keychain as usual — `taurus
key set anthropic` — or in a variable named by `api_key_env`.

**Neither needs a `context_length`,** and neither should be given one except as
a fallback. Anthropic reports a window and a capability tree per model, so
Taurus asks; Gemini reports a window in its model listing. A configured value
that disagrees with the model is how a conversation compacts at the wrong
moment, so the field is offered in Settings as "only used if the backend will
not report its own window" and left empty by default.

**Prompt caching is on by default on Anthropic.** The system prompt and tool
schemas are exactly the fixed overhead [`taurus usage`](#the-context-window)
exists to report — re-sent on every iteration of every turn — and this is the
one backend here that will serve them back at about a tenth of the price. Two
breakpoints of the four allowed: one after the system prompt, which also covers
the tools rendered before it, and one on the newest turn, so the cached prefix
grows with the conversation rather than resetting each iteration. Cached tokens
are counted into the input total, so a well-cached turn reports what the request
carried rather than only the part that missed.

**Thinking is left to the model by default.** Sending no `thinking` field is the
only setting valid on every model that API has served — the newer ones reason by
default and the older ones do not, and neither rejects a request that says
nothing. `"thinking": "adaptive"` or `"disabled"` overrides it, and the wrong
one is a 400 rather than a preference, which is why it is not guessed.

Reasoning blocks are replayed with the signature the provider issued them under.
That is not a nicety: a turn that reasoned and then called a tool is only legal
on the next request if its thinking comes back signed and unedited, so a
signature that did not survive the stream is a rejected request one turn later.

**Gemini's schemas are sanitized on the way out.** It accepts an OpenAPI 3
subset and refuses a request outright on a keyword it does not know, with an
error naming the tool rather than the offending word — so `$schema`, `title`,
`additionalProperties`, and the integer-width `format`s that `schemars` emits
are stripped at every level of every tool schema. Its tool calls carry no ids,
so Taurus synthesizes them and resolves them back to names on the way out;
without that, two calls to the same tool in one turn would be indistinguishable
and so would their results.

### Azure OpenAI, and gateways in front of it

Azure is an OpenAI-compatible backend that disagrees about one thing: where the
key goes. OpenAI and everything imitating it read `Authorization: Bearer`;
Azure OpenAI reads `api-key`, and an Azure API Management gateway reads
`Ocp-Apim-Subscription-Key`. Both are bare — a `Bearer ` in front of the value
produces a 401 that looks exactly like a wrong key.

`api_key_header` names the header. The key is sent raw in it, with no scheme
prefix:

```jsonc
{
  "id": "apim",
  "kind": "open_ai_compatible",
  "base_url": "https://my-gateway.azure-api.net",
  "api_prefix": "/openai/v1",
  "api_key_env": "APIM_SUBSCRIPTION_KEY",
  "api_key_header": "Ocp-Apim-Subscription-Key",
  "models": ["gpt-4o", "gpt-4o-mini", "o3"],
  "default_model": "gpt-4o",
  "context_length": 128000
}
```

Leaving it unset keeps bearer auth, so nothing else changes. There is
deliberately no separate setting for the scheme: naming `Authorization` sends
the key bare in that header, which is the only other shape a gateway asks for.
The value is marked sensitive, so a subscription key cannot reach a debug log
through the header map.

Two other fields matter more here than elsewhere:

- **`api_prefix`.** Azure's OpenAI-shaped surface lives under `/openai/v1`,
  where the model goes in the request body. Its older data plane puts the
  deployment name in the path and requires an `api-version` query parameter;
  Taurus cannot express either, so point it at the `/openai/v1` route, or at an
  APIM route whose policy supplies them.
- **`models`.** A gateway need not expose `/v1/models`, and plenty of the ones
  that do answer with an inventory rather than an entitlement — every model the
  vendor sells, including the ones this key cannot call. Naming models here
  replaces that listing outright: what is listed is what the picker offers, and
  no request is made to find out. Leave it out and Taurus asks, which is right
  for Ollama and for any endpoint that answers usefully.

  An entry is either a bare id or an object, so the common case stays one word:

  ```jsonc
  "models": [
    "gpt-4o",
    { "id": "llama-3.1-8b", "context_length": 8192, "native_tools": false }
  ]
  ```

  The overrides matter because an OpenAI-compatible endpoint reports no
  capabilities at all, and one gateway commonly fronts models that share
  neither a context window nor tool support — told the provider-wide 128000
  above, an 8k model compacts tens of thousands of tokens too late. Anything
  left unset inherits the provider's own value, so a bare id means exactly what
  it did before these existed.

  A workspace layer *replaces* this list rather than adding to it. Appending
  could not express dropping a model, and a workspace that names models is
  saying which ones it wants.

- **`default_model`.** Which of them a new conversation starts on. Optional —
  the first model is used otherwise. It also still works alone, without
  `models`, which is all a single-model gateway ever needed. With neither, and
  no listing, the error says so instead of reporting an unreachable backend.

### Intel hardware, and other backends

Taurus never touches an accelerator. Every provider crate is JSON over HTTP —
there is no inference code and no device selection in this repository, so which
of your CPU, GPU, or NPU runs a model is decided entirely by the server you
point it at.

That matters because **stock Ollama on Intel runs on the CPU.** Getting Intel
acceleration means installing a different server, not configuring Taurus:

| Server | Intel target | Taurus config |
| --- | --- | --- |
| [OpenVINO Model Server](https://docs.openvino.ai/2025/model-server/ovms_what_is_openvino_model_server.html) | CPU, Arc GPU, Core Ultra NPU | `open_ai_compatible` |
| [IPEX-LLM's Ollama build](https://github.com/intel/ipex-llm) | Arc and integrated GPUs | `ollama`, unchanged |
| llama.cpp built with the SYCL backend | Arc and integrated GPUs | `open_ai_compatible` |

#### OpenVINO Model Server

Serve a model, choosing the device with `--target_device`:

```bash
docker run --rm -p 8000:8000 \
  -v $(pwd)/models:/models openvino/model_server:latest \
  --model_path /models/Qwen3-8B-int4-ov --model_name qwen3 \
  --task text_generation --target_device NPU --rest_port 8000
```

Models must be in OpenVINO IR format — export with
`optimum-cli export openvino`, or pull a pre-converted one from the `OpenVINO`
org on HuggingFace.

```jsonc
{
  "id": "openvino",
  "kind": "open_ai_compatible",
  "base_url": "http://localhost:8000",
  "context_length": 8192,
  "native_tools": true
}
```

Three settings decide whether this works, and getting them wrong fails in ways
that do not look like configuration errors:

- **`api_prefix`.** OVMS served the OpenAI routes under `/v3` until 2026.3
  added `/v1` as an alias. On anything earlier, add `"api_prefix": "/v3"` — the
  symptom otherwise is a flat `404` from a server that is running fine.
- **`context_length`.** OVMS caps prompts at 8k tokens on NPU. This value is
  what drives compaction, and it cannot be probed over the OpenAI API, so it
  defaults to 128000 — roughly sixteen times too high for an NPU. History would
  never be compacted before the server began rejecting requests, and the
  failure surfaces as a provider error rather than as "this conversation got
  too long".
- **`native_tools`.** Tool calling in OVMS needs a per-model *tool parser* —
  Qwen3, Hermes3, Llama3, Mistral-7B-v0.3, phi-4-mini and others have one.
  Serving a model without one and leaving `native_tools` at its default of true
  gives a model that narrates tool calls it never actually makes. Set it to
  `false` and Taurus switches to prompted tool calling, the same fallback it
  uses for `gemma3` on Ollama.

`api_prefix` is not OpenVINO-specific: any server behind a reverse proxy that
mounts the API elsewhere needs it, and `""` puts the routes directly on the
base URL.

Layering is per key, not per file, so an override states only what it changes:

```jsonc
// <workspace>/.taurus/providers.json — point one provider at a different box
[{ "id": "ollama", "base_url": "http://gpu-box:11434" }]
```

`kind`, `api_key_env`, `api_key_header`, and the capability overrides are
inherited from the global entry with the same `id`. An entry whose `id` is new
to this layer is
added instead, and then needs a `kind` and a `base_url` of its own. Anything
unresolvable — a malformed layer, an override with no `kind`, an MCP toggle
naming a server nothing defines — is reported as a startup problem rather than
silently dropped, and the other layer still loads.

Because the workspace layer belongs to a directory you can change at any time,
all of it is re-resolved on every workspace switch, not just at startup.

Two values are deliberately not layered. `last_workspace` is written globally
only — a pointer to a workspace, stored inside the workspace it names, could
only ever point at itself. Edits from the app's provider settings also write
globally: the workspace layer is meant to be hand-written and version-controlled,
and round-tripping the merged view into it would bake every inherited value
into the project file.

## Development

```bash
cargo test --workspace     # 789 tests
pnpm test                  # transcript reducer, replay, settings, rewind, diffs
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# The README's screenshots. Needs Chrome or Chromium; nothing else does.
pnpm screenshots

# TypeScript types are generated from Rust; regenerate after changing a payload.
# `src/bindings` is not committed, so this is also the first thing a fresh clone
# needs — without it `pnpm build` cannot find a single frontend type.
pnpm bindings
```

### The app icon

Every icon the bundles use is generated from `app-icon.svg` at the repository
root, which is the same mark the rail draws — the `Logo` in
`src/components/icons.tsx`, on the same grid, with the colours resolved because
Finder and Explorer do not know what `var(--accent)` means.

```bash
pnpm tauri icon app-icon.svg     # regenerates src-tauri/icons/
```

Both halves of that matter. `src-tauri/icons/` held a flat purple square for
long enough to ship in a release, which nothing caught because no test can look
at a picture — so the mark and the icon are now one file apart rather than two
unrelated drawings. The mobile output the generator also writes is deleted;
there is no Android or iOS project here to consume it.

On Windows the executable's icon does not come from the bundler at all.
`tauri-build` reads the first `.ico` in `bundle.icon` and embeds it as resource
`32512` while compiling, which is why a plain `cargo build --release -p
taurus-app` — what CI runs, producing `taurus-app.exe` — carries the icon
without any bundling step. Keep an `.ico` first in that list.

### The README's screenshots

```bash
pnpm screenshots     # rewrites docs/screenshots/*.png
```

The images are the real frontend — the real `App`, the real store, the real
stylesheet — served on its own and driven in headless Chrome, with the Tauri IPC
bridge answering fixtures. They are not photographs of a running desktop app:
there is no window chrome, and the conversation is canned. That trade is
deliberate, and it is the same argument the app icon makes above. A hand-taken
screenshot is a picture nothing can check, so it goes stale silently and ends up
documenting a version of the app that no longer exists; one that regenerates
from the components in `src/` cannot drift further than the last person who ran
the command.

The fixtures are the `UiEvent` stream a turn emits, folded by the store's own
reducer — not hand-built view state. A screenshot assembled around the
components would look right while the code that feeds them was broken.

Regenerate after a visible UI change, and eyeball the result: this is the only
check in the repository that looks at pixels, and reviewing the PNG in the diff
is the whole of it.

### Live checks

These run against a real Ollama server and are the fastest way to confirm a
change did not break the parts that unit tests cannot reach.

```bash
# One provider, one turn, one tool call.
cargo run -p taurus-provider-ollama --example smoke -- qwen3.6:27b
cargo run -p taurus-provider-ollama --example smoke -- gemma3      # prompted fallback

# The OpenAI adapter, against Ollama's own /v1 endpoint.
cargo run -p taurus-provider-openai --example smoke -- llama3.2:latest

# The hosted adapters. Each prints the capabilities it probed before the turn,
# which is the half of these two that has no local equivalent.
ANTHROPIC_API_KEY=… cargo run -p taurus-provider-anthropic --example smoke -- claude-opus-5
GEMINI_API_KEY=…    cargo run -p taurus-provider-gemini    --example smoke -- gemini-2.5-pro

# The whole harness: read files, write a file, report what happened.
cargo run -p taurus-core --example e2e -- qwen3.6:27b

# Skill authoring: propose, validate, save, rediscover.
cargo run -p taurus-skills --example synthesis -- qwen3.6:27b

# Delegation: define a scoped agent, delegate to it, prove it stayed in scope.
cargo run -p taurus-agents --example delegate -- qwen3.6:27b

# MCP: connect, list tools, call one through the registry.
cargo run -p taurus-mcp --example probe -- path/to/mcp.json

# Web: one real search, then fetch the first result it returns.
cargo run -p taurus-web --example probe -- ~/.taurus/search.json "rust async book"

# What a sweep costs on a real workspace, and that it stays quiet when nothing
# changed. Needs no provider. Run it on something large before touching the
# caps in `sweep.rs` — every command pays this twice.
cargo run -p taurus-tools --example sweep -- .
```

The drawn results have no example of their own, because the check worth making
is that a model reaches for them unprompted and that what it sends survives the
trip. One turn does both:

```bash
# Draws a table on stdout, then answers in prose beside it.
taurus run "Four crates and their build times: taurus-core 42.1s, taurus-tauri
31.7s, taurus-mcp 18.4s, taurus-agents 11.9s. Show me the comparison."

# No terminal to ask on, so this must decide and say so rather than hang.
echo "" | taurus run "Ask me whether to rename everywhere or only in settings,
then act on the answer."
```

The CLI doubles as a live check on the whole stack:

```bash
taurus tools                    # what the agent can reach
taurus skills check             # non-zero exit if a skill is broken or degraded
taurus agents check             # non-zero exit if an agent will not load or run as written
taurus mcp                      # non-zero exit if a server failed to connect
taurus key status               # where each provider's API key comes from
```

`skills check`, `agents check`, and `mcp` are meant for CI on a repository that
ships its own `.taurus` directory.

## Known gaps

- **A rewind does not cover ignored directories.** A file an ignore rule
  excludes by name is covered; everything under a directory an ignore rule
  excludes is not, so a command that rewrites something in `target/` or
  `node_modules/` is neither listed nor restorable. Widening it means indexing
  those before every command, which is not affordable, and having a rewind
  delete build output, which is not wanted. See
  [Rewinding a turn](#rewinding-a-turn).
- **A rewind reports git state, it does not put it back.** `.git` is left out
  of the walk, so undoing a turn that ran `git checkout` or `git reset --hard`
  restores the file contents while leaving `HEAD` and the index where the
  command moved them — a tree that matches neither commit. The turn now says so
  when it happens, and points at `git reflog`, but covering it properly means
  snapshotting the object store, which is its own feature. The warning also
  reaches you at the moment the command runs rather than at the moment you
  reach for undo; carrying it into the checkpoint log means a record shape and
  a format version, and has not been done. Staging is unreported as well — see
  [Rewinding a turn](#rewinding-a-turn) for why the index is deliberately not
  watched.
- **A change that moves neither length nor timestamp is invisible.** The same
  walk compares size and modification time, which is what `make` and `rsync`
  have always compared. On a filesystem with nanosecond timestamps defeating it
  takes deliberate effort; on one with coarse timestamps, a command that
  rewrites a file to the same length within the same tick would slip through.
  Closing it means reading every file twice per command.
- **A pty command's stdout and stderr cannot be told apart.** A terminal has one
  stream, so `pty: true` gives up the `[stderr]` split the piped path reports.
  That is the format rather than the implementation, and it is why the pty is
  opt-in rather than the default. Output streams to the transcript as it is
  produced on both paths — batched every 100ms, kept as a bounded scrollback,
  and dropped from the *display* rather than allowed to stall the child if the
  UI falls behind. What the model receives is always the complete output; only
  what you are watching scroll past can skip.
- **A pty command answers prompts it was given, not prompts it was not.** `stdin`
  is written up front and closed, so a program that asks something unanticipated
  still waits for the timeout. Driving a genuine back-and-forth would mean
  keeping the turn open on a running child and deciding what the model is
  allowed to type into it, which is a larger surface than this opens.
- **Reasoning the provider returns redacted cannot be replayed.** Anthropic
  signs its thinking blocks and requires them back unedited; a redacted one has
  no signature this harness can carry, so it is left out of the next request.
  Where that matters the API says so explicitly rather than failing quietly, but
  it is a turn that has to be retried. Carrying the encrypted form would mean a
  second shape in the normalized types for one provider's edge case.
- **An instructions file is read, not watched.** `AGENTS.md` is re-read on every
  reload and on every workspace switch, so an edit lands on the next reload
  rather than the next turn. The Skills drawer's Rescan is the manual way; a
  file watcher is the same surface — config reloads racing a running turn — that
  the agent roster deliberately leaves closed.
- **A diff is shown for `write_file` and `edit_file` and nothing else.** A
  command line has no before-and-after to compute, which is exactly why
  `run_command` is swept afterwards rather than predicted. So the most
  consequential writes in a session — the ones a script made — are still
  approved on the command line alone, and only become visible in the **Changes**
  drawer once they have happened.
- **A sub-agent's answer is summarized, not streamed.** Its tool calls now
  appear under the delegation card as it makes them, so a long delegation looks
  alive rather than hung, but its reasoning and prose stay inside the child.
  That part is deliberate: the parent asked for a conclusion, and a second
  conversation inlined into the transcript is what delegation exists to avoid.
- **A custom agent's roster is frozen for the turn.** The set of sub-agents is
  snapshotted when a turn starts, so an agent file saved mid-turn is not visible
  until the next one. The drawer rescans on open, which covers editing; a file
  watcher would close the rest, and is a surface — config reloads racing a
  running turn — worth opening deliberately rather than as a side effect.
- **A proposed agent's system prompt is reviewed by eye, and nothing else.**
  `propose_agent` checks the shape — the name, the description, the tool scope,
  whether it duplicates an existing agent — but the prompt itself is prose, and
  prose that will steer a delegate on every future turn. That is the same
  exposure `propose_skill` has always had, and it has the same answer: the card
  shows it in full, unelided and editable, and nothing is written until you
  approve it. There is no check that reads what it says. See
  [Sub-agents](#sub-agents).
- **Taurus will not install an MCP server for you.** `draft_mcp_server` writes
  a block to paste; adding it is yours to do. The command line is the whole of
  what a review could show, and it does not say what the program does, so this
  is a limit rather than a to-do. See [MCP servers](#mcp-servers).
- **An agent's tools narrow what it is offered, not what it may do.** Every call
  a child makes goes through the same permission engine as the parent's, so
  `tools:` is a scope, not a sandbox. A per-agent permission policy would be a
  second thing to keep in step with the first, and is not there.
- **Stall detection needs an exact repeat.** Alternating between two dead ends
  is now caught, but the calls have to match argument for argument. A model
  asking the same unanswerable question in three slightly different ways —
  reading a missing file by three spellings of its path — is making no more
  progress than one asking it identically, and nothing here notices. Judging
  that would mean deciding when two calls are *near* enough to be the same
  mistake, which is a guess the iteration ceiling makes unnecessary. See
  [When a turn stops](#when-a-turn-stops).
- **`fetch_url` reads the HTML it is served.** No JavaScript runs, so a page
  that renders its content client-side comes back near-empty. Closing this
  means shipping a browser engine, so it is a limit rather than a to-do.
- **`fetch_url`'s address check does not survive a proxy.** Loopback and
  private-network addresses are refused, and the check now runs inside the
  client that connects, so a name cannot answer publicly for the check and
  privately for the connection. An HTTP proxy resolves the name at its end
  though, so a request routed through one reaches a destination Taurus never
  sees. Taurus configures no proxy, but reqwest reads `HTTP_PROXY` and the
  system settings, and refusing to work behind a corporate proxy would cost
  more than this buys. `"allow_private_hosts": true` in `search.json` turns
  the check off deliberately.

## License

MIT. See [LICENSE](LICENSE).
