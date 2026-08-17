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

## Skills

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

A project agent shadows a personal one of the same name, and either shadows a
built-in — so a `explorer.md` of your own replaces the shipped explorer rather
than sitting beside it. The drawer says on the row when that has happened.

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
agent does not restart every MCP server — and saving a server, in the same
spirit, reconnects the servers without rescanning the roster. It is not usable in the turn that
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
