# Capabilities

<sub>[← Taurus AI Shell](../README.md)</sub>

What the agent can reach for, and what it writes down. Every one of these
works the same in the desktop app and the CLI, because none of it lives in a
frontend — see [How it is put together](../README.md#how-it-is-put-together).

## Instructions

A skill is a procedure the model loads when it needs one. Instructions are the
opposite: a short standing brief that applies to every turn — this project's
conventions, how you want work done, what not to touch. Taurus reads the files
you already have rather than asking for a seventh copy, on the same rule the
skill library follows. Seven locations, lowest precedence first:

```
~/.agents/AGENTS.md         <workspace>/AGENTS.md
~/.claude/CLAUDE.md         <workspace>/CLAUDE.md
~/.taurus/TAURUS.md         <workspace>/.taurus/TAURUS.md
                            <workspace>/.github/copilot-instructions.md
```

GitHub Copilot's repository brief is exactly this: one file, whole workspace,
every turn. It has no personal counterpart to read — Copilot keeps a person's
standing rules in the scoped files below rather than in a single file.

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

**Copilot's scoped instructions are read, with their scope stated.** A
`*.instructions.md` file under `.github/instructions` or `~/.copilot/instructions`
declares an `applyTo` glob, and Copilot attaches it when it is about to touch a
matching file. Taurus has no such moment — a brief is assembled once per turn,
before anyone knows which files the turn will read — so the glob is carried into
the prompt as a sentence and the model applies it when it applies:

```
## rust.instructions.md (project, applies to files matching `**/*.rs`)

Never use unwrap in library code.
```

That is weaker than Copilot's rule and stronger than dropping the file, which
are the only other two options. Both folders are searched recursively, and the
frontmatter is stripped rather than read aloud.

A file with **no `applyTo` is not carried**, and says so in the Skills drawer.
Copilot does not apply those automatically either — they are for pulling into a
request by hand — so carrying one into every turn would be Taurus asserting
something about the file that the tool it was written for does not. Giving it
`applyTo: "**"` makes it a standing brief.

Because a directory of these can grow without anyone noticing, the total is
budgeted: past 24 KB across all briefs, the drawer says so. Every byte is paid
on every request of every turn.

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

**An edit lands on your next message.** Taurus re-reads the brief at the start
of each turn — including any file it imports, which is where the whole of a
brief often lives — so editing `AGENTS.md` beside an open conversation works the
way it looks like it should. It does not *watch* the files: a watcher fires
whenever an editor happens to save, which is routinely the middle of a running
turn, and the brief a turn was given has to be the one it started with. A turn
boundary is the same change one turn later, without anything being swapped
underneath work in progress.

Checking costs a `stat` per file and re-reading only happens when one moved, so
the common case — nothing changed — is a few microseconds per message. The
comparison is length and modification time, the same one the sweep makes about
the workspace and blind in the same place: a rewrite to the same length within
one filesystem tick waits for the next change to be noticed.

The brief lands directly after the harness's own rules and before the skill
catalog. That ordering is the design: a brief saying "ask before touching the
database" argues with "keep going until the task is done", and a small model
settles a contradiction by recency — so the brief comes second, where it wins.

## Memory

Instructions are what you tell it. Memory is what it tells the next
conversation.

A session ends where it ends. The transcript is on disk and can be reopened, but
the conversation after it starts with none of that — so the first thing you do
the next morning is explain, again, what was being done and how far it got.
Nothing tells it that the auth refactor is half applied, or that the flaky test
was tracked to a clock and not to the code.

So the model can write a note, with `remember`, when it works out something that
outlives the conversation: work left half-done and where it stopped, a decision
and the reason for it, a dead end worth not repeating. Notes are kept per
workspace and read into the system prompt of every later conversation there,
newest first, under a heading saying what they are and that they were true when
written rather than necessarily now.

```
~/.taurus/memory/<workspace>/notes.jsonl
```

Beside the transcripts and checkpoints, keyed the same way, for the same reason:
a note is prose about the contents of your project, and a file in the project is
a file that gets committed. It is one JSON object per line and is meant to be
readable — a line you write yourself, without the `id` the model's own notes
carry, loads like any other.

**Nothing is written behind your back, and nothing is written *for* you.** A
note is not a proposal you approve, the way a skill is — a dialog on something
written this often is one you learn to dismiss. Instead it happens where you can
see it: the call appears in the transcript as it is made, marked as a note
rather than folded in with the reads, and every note is listed in the **Memory**
drawer with the conversation it came from and a button to forget it. The same
list is `taurus notes list`, and the same button is `taurus notes forget <id>`.

A note is capped at 2 KB and refused rather than truncated past it, because a
note cut off mid-sentence still reads as a fact. The prompt carries the newest
twelve under a 4 KB ceiling, and the file keeps the newest 200 — the same
bargain the standing brief makes, for the same reason: these bytes are paid on
every request of every turn.

A conversation is never handed its own notes. They are already in its
transcript, and repeating them back under a heading that says they came from an
earlier conversation would be the harness telling the model something untrue
about where they came from.

## Skills

A skill is a `SKILL.md` with YAML frontmatter plus optional bundled scripts, in
the format defined by the [Agent Skills specification](https://agentskills.io/specification).
Taurus reads the shared locations as well as its own, so a skill installed by
another client works here without being copied. Eight directories, lowest
precedence first:

```
~/.agents/skills            <workspace>/.agents/skills
~/.claude/skills            <workspace>/.claude/skills
~/.copilot/skills           <workspace>/.github/skills
~/.taurus/skills            <workspace>/.taurus/skills
```

GitHub Copilot reads the same specification, so its skills are already skills
Taurus understands and the whole cost is that row. It is the one origin whose
two directories are not named the same — a repository's Copilot customizations
live in the folder GitHub already reads rather than a dotdir of Copilot's own —
so the drawer's tag says `.github` on a project skill and `.copilot` on a
personal one.

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
dark mode toggle` — in the app and in `taurus run` alike. The harness fills the
skill's `$ARGUMENTS` placeholder with the rest of the line and hands the model
the procedure instead of the command; a skill with no placeholder gets the text
appended under a heading rather than losing it. Sub-agents share the same `/`
namespace — see [Slash commands](#slash-commands).

Two frontmatter flags decide the ways in. `disable-model-invocation: true`
keeps a skill out of the prompt catalog while leaving it runnable by name — for
procedures that should run when a person asks and not before. `user-invocable:
false` does the reverse. Both are optional and both default to available.

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

## Sub-agents

A turn can hand a self-contained job to a sub-agent: its own conversation, its
own context window, and a narrower set of tools. The parent sees only the
child's conclusion, so a search that reads thirty files costs it one paragraph.

Three ship with the harness, and the split between them is a question the parent
can answer about its own task before it delegates:

| Agent | Use it when | Scoped to |
| --- | --- | --- |
| `explorer` | The answer is in the code and only needs reading. | `read_file`, `list_dir`, `glob`, `grep`, `load_skill` |
| `worker` | You can dictate the edit exactly. | Whatever the main agent has |
| `coder` | Someone has to look at the code and decide. | The file tools, `grep`/`glob`, `run_command`, `load_skill` |

`coder` is the one that checks its own work: it is told to read around the
change before writing it, and to build it or run the tests afterwards and report
what it ran. That is also why it is scoped to a named list rather than
inheriting — an agent that advertises "builds or tests it" and quietly also
holds a web client and your MCP servers is advertising a different thing. The
model reaches any of them through `spawn_subagent`.

`coder` and `worker` overlap at the boundary, because real tasks do. What keeps
them apart is who decides: hand `worker` a decision it was not given and it is
told to stop and say what is missing rather than guess.

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

Agents are read from four directories, the same shape the skill library uses —
the borrowed locations first, Taurus's own last:

```
~/.claude/agents/<name>.md           <workspace>/.claude/agents/<name>.md
~/.copilot/agents/<name>.agent.md    <workspace>/.github/agents/<name>.agent.md
~/.taurus/agents/<name>.md           <workspace>/.taurus/agents/<name>.md
```

The first two rows are Claude's and GitHub Copilot's, read for the reason
`.claude/skills` is: an agent written for another tool is frontmatter and a
markdown body that is its system prompt, which is what one written for Taurus
is, so reading them costs directories rather than a second parser. Copilot's
doubled extension is understood — `reviewer.agent.md` is the agent `reviewer`,
not one called `reviewer.agent`. Frontmatter keys Taurus does not have are
ignored rather than honoured.

A project agent shadows a personal one of the same name, and either shadows a
built-in — so a `explorer.md` of your own replaces the shipped explorer rather
than sitting beside it. Within a tier, yours wins over a borrowed one, which is
how you override an agent you did not write without editing it. The drawer says
on the row when that has happened.

**A borrowed file is read and never written.** Retuning `max_iterations` on a
Copilot agent saves a Taurus-owned copy beside it and shadows the original,
exactly as editing a built-in does — and the field says so before you touch it.
That is not tidiness: the file is usually committed, Copilot is still reading it,
and its frontmatter carries keys Taurus has never heard of. Taurus rewrites a
file from the fields it knows, so editing one in place would silently delete
every `handoffs:` and `hooks:` in it. The copy lands in the same tier as the
file it overrides, because a user-tier copy of a project-tier agent would sit
underneath the thing it was meant to replace.

**A new or edited agent is available on your next message.** The directories are
checked at the start of each turn and rescanned only when something in them
moved, so writing `reviewer.md` in an editor and delegating to it in the next
message works without a reload or a trip to the drawer. The roster is still
frozen *within* a turn — a file saved while one is running is not visible to it
— which is deliberate: a turn has to delegate against the set of agents it
started with, and that is exactly what a file watcher could not promise. The
drawer's **Rescan** is still there for the moment you want it now rather than on
the next message.

One place lags by design: the `/` command menu lists what the last scan found,
because it is redrawn on every keystroke and taking config locks there is how a
reload comes to deadlock against typing. Typing the agent's name in full works
straight away — the name is resolved against a fresh roster — and the menu
catches up after the next message.

`max_iterations:` is how many model/tool round trips this agent gets before it
is stopped, between 1 and 100. It is editable on the agent's card in the Agents
drawer as well as in the file — that edit rewrites the file in place, so
`model:` and anything else you set by hand survives it. Editing a built-in's
limit has nowhere to write, so it saves a copy you own into `~/.taurus/agents`
that shadows the built-in; the card says so before you change it. The same
ceiling governs the conversation that delegates — see
[the iteration ceiling](working-with-it.md#when-a-turn-stops), which is a
separate number, in Settings › Behavior.

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

Every delegate keeps a transcript of its own, written as it runs, in a directory
named for the conversation that spawned it:

```text
~/.taurus/sessions/<workspace>/<id>.jsonl                    the conversation
~/.taurus/sessions/<workspace>/<id>/subagents/agent-*.jsonl  what it delegated
```

The parent's transcript still records a delegation as what it is — one call, one
paragraph back — while the reading, the dead ends and the reasoning behind that
paragraph stay somewhere they can be found. A delegate is not a conversation
somebody had, so it never appears in the session list, and deleting a
conversation deletes its delegates with it.

In the app the delegation's row offers to open it, in a drawer beside the
conversation rather than inside it — while the call is still running, which is
when a delegation that looks stuck is worth looking into, and afterwards when
the paragraph it returned is thinner than expected. It opens read-only: a
delegate's conversation happened inside somebody else's turn, and there is
nothing there to continue.

```bash
taurus agents list          # the roster, what each is scoped to, what it costs
taurus agents check         # non-zero if an agent will not load or cannot run as written
taurus sessions --agents ID # what one conversation delegated, and where it was written
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
agent does not restart every MCP server — and saving a server, in the same
spirit, reconnects the servers and rescans the roster only when the set of tools
those servers offer actually changed, which is the one way a server can affect an
agent. It is not usable in the turn that
proposed it — a turn's roster is frozen when it starts — and the tool result
says so, rather than letting the model spend a round trip finding out.

## Slash commands

One `/` namespace covers both libraries, in the app and in `taurus run` alike:

```
/speckit-specify add a dark mode toggle    # runs that skill's procedure
/reviewer check the auth module            # hands the job to that sub-agent
```

The composer completes as you type `/` and tags each row **skill** or **agent**,
because the two do different things with the rest of the line. A skill's
procedure replaces your message. An agent's name becomes an instruction to
delegate: the turn calls `spawn_subagent` with the line as the task, and what
comes back is the child's conclusion rather than the thirty files it read.
Delegation stays a tool call rather than a separate code path, so a command runs
exactly the agent the model would have run on its own — same tool scoping, same
permission gate, same depth cap.

`/explorer` with nothing after it points the agent at what the conversation has
already established, which is what "now do that part with the explorer" means.
Both built-ins are reachable this way on a machine with no agents directory.

A name held by both a skill and an agent runs the skill. That is not a judgement
about which is more useful — it is that a command which quietly starts doing
something else is worse than a name that is awkward to reach. Rename one of the
two if you want both. A model-only skill (`user-invocable: false`) does not
reserve its name, so an agent behind one is still reachable.

A name nothing matches is reported to you rather than sent, with the near misses
from both rosters. Turning `spawn_subagent` off in `disabled_tools` takes agents
out of the menu, and typing one anyway says that rather than "no such command".

Ordinary text that begins with a slash is never treated as a command:
`/usr/bin/env is portable` is sent as written. A command has to name a skill or
an agent, start with a letter, and be followed by a space or nothing.
