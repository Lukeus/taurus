# Capabilities

<sub>[← Taurus AI Shell](../README.md)</sub>

What the agent can reach for, and what it writes down. All of it works the
same in the desktop app and the CLI, because none of it lives in a frontend.
See [How it is put together](../README.md#how-it-is-put-together).

## Instructions

A skill is a procedure the model loads when it needs one. Instructions are the
opposite: a short standing brief for every turn, covering your project's
conventions, how you want work done, and what not to touch.

Like the skill library, Taurus reads the files you already have instead of
asking for a seventh copy. Seven locations, lowest precedence first:

```
~/.agents/AGENTS.md         <workspace>/AGENTS.md
~/.claude/CLAUDE.md         <workspace>/CLAUDE.md
~/.taurus/TAURUS.md         <workspace>/.taurus/TAURUS.md
                            <workspace>/.github/copilot-instructions.md
```

GitHub Copilot's repository brief is exactly this: one file, whole workspace,
every turn. There's no personal counterpart to read. Copilot keeps your
standing rules in the scoped files below, not in a single file.

The project files sit at the repository root, not in a dotdir, because that's
where a repo's brief lives: `AGENTS.md` beside the README. Looking anywhere
else would find nothing in the projects this is for.

**They accumulate rather than shadow.** That's the one deliberate difference
from skills. Two skills named `deploy` are rival answers to one question, so
the project's wins. But "I prefer terse commit messages" and "this repo pins
its toolchain" are both true at once, and dropping either would be a silent
loss.

Each file is labeled in the prompt with its source, so the model can tell a
personal preference from a project requirement. Where they disagree, the
section says the project's wins.

Two files with identical bytes are read once. `CLAUDE.md` symlinked to
`AGENTS.md` is the common setup, and a rule the model sees twice gets weighted
twice.

**Copilot's scoped instructions are read, with their scope stated.** A
`*.instructions.md` file under `.github/instructions` or
`~/.copilot/instructions` declares an `applyTo` glob. Copilot attaches it when
it's about to touch a matching file. Taurus has no such moment: it builds the
brief once per turn, before anyone knows which files the turn will read. So
the glob goes into the prompt as a sentence, and the model applies it when it
applies:

```
## rust.instructions.md (project, applies to files matching `**/*.rs`)

Never use unwrap in library code.
```

That's weaker than Copilot's rule and stronger than dropping the file, and
those are the only other two options. Both folders are searched recursively.
The frontmatter is stripped, not read aloud.

A file with **no `applyTo` is not carried**, and the Skills drawer says so.
Copilot doesn't apply those automatically either. They're for pulling into a
request by hand. Carrying one into every turn would mean Taurus claiming
something about the file that the tool it was written for doesn't. Give it
`applyTo: "**"` to make it a standing brief.

A directory of these can grow without anyone noticing, so the total is
budgeted. Past 24 KB across all briefs, the drawer says so. Every byte is paid
on every request of every turn.

**`@path` imports are resolved**, one level deep. Claude Code's format lets a
file be a list of pointers, and real ones are. A global `CLAUDE.md` whose
entire content is `@RTK.md` would otherwise read as a single meaningless line.

A line counts as an import only when the whole line is `@` followed by a path.
So `Ask @alice before releasing` is prose and stays prose. An import of a
missing file is reported, not passed through, because a pointer at nothing
tells the model less than nothing.

A file longer than 12 KB is cut on a line boundary, and both the prompt and
the Skills drawer say so. Without the cap, a checked-in handbook could spend an
8k model's whole context before it read a line of code.

**An edit lands on your next message.** Taurus re-reads the brief at the
start of every turn, along with any file it imports, which is often where the
whole brief lives. So you can edit `AGENTS.md` with a conversation open and it
just works.

It doesn't *watch* the files. A watcher fires whenever your editor saves,
which is often mid-turn, and a turn has to finish on the brief it started
with. Waiting for the next turn gets you the same change one message later,
without swapping anything out from under running work.

The check costs a `stat` per file, and Taurus only re-reads a file that moved.
When nothing changed, which is the usual case, that's a few microseconds per
message. It compares length and modification time, the same test the sweep
uses on the workspace, and it has the same blind spot: a rewrite to the same
length within one filesystem tick isn't noticed until the next change.

The brief goes directly after the harness's own rules and before the skill
catalog. That order is deliberate. A brief saying "ask before touching the
database" argues with "keep going until the task is done", and a small model
settles a contradiction by recency. So the brief comes second, where it wins.

## Memory

Instructions are what you tell it. Memory is what it tells the next
conversation.

A session ends where it ends. You can reopen the transcript from disk, but the
next conversation starts with none of it. So next morning you'd start by
explaining, again, what you were doing and how far it got. Nothing tells it
the auth refactor is half applied, or that the flaky test was traced to a
clock and not to the code.

So the model can write a note with `remember` when it works out something that
outlives the conversation:

- work left half-done, and where it stopped
- a decision, and the reason for it
- a dead end worth not repeating

Notes are kept per workspace. They're read into the system prompt of every
later conversation there, newest first, under a heading that says what they
are. The heading also says they were true when written, not necessarily now.

```
~/.taurus/memory/<workspace>/notes.jsonl
```

The file sits beside the transcripts and checkpoints, keyed the same way, for
the same reason. A note is prose about the contents of your project, and a
file in the project gets committed. It's one JSON object per line and meant to
be readable. A line you write yourself, without the `id` the model's own notes
carry, loads like any other.

**Nothing is written behind your back, and nothing is written *for* you.** A
note isn't a proposal you approve, the way a skill is. You'd learn to dismiss
a dialog on something written this often. Instead it happens where you can
see it:

- The call appears in the transcript as it's made, marked as a note rather
  than folded in with the reads.
- Every note is listed in the **Memory** drawer with the conversation it came
  from and a button to forget it.

The same list is `taurus notes list`, and the same button is
`taurus notes forget <id>`.

Notes are capped at 2 KB. A longer one is refused, not truncated, because a
note cut off mid-sentence still reads like a fact. The prompt carries the
newest twelve under a 4 KB ceiling, and the file keeps the newest 200. That's
the same bargain the standing brief makes, for the same reason: these bytes
are paid on every request of every turn.

A conversation is never handed its own notes. They're already in its
transcript. Repeating them under a heading that says they came from an earlier
conversation would mean the harness telling the model something untrue.

## Skills

A skill is a `SKILL.md` with YAML frontmatter plus optional bundled scripts, in
the format defined by the [Agent Skills specification](https://agentskills.io/specification).
Taurus reads the shared locations as well as its own, so a skill another
client installed works here without copying. Eight directories, lowest
precedence first:

```
~/.agents/skills            <workspace>/.agents/skills
~/.claude/skills            <workspace>/.claude/skills
~/.copilot/skills           <workspace>/.github/skills
~/.taurus/skills            <workspace>/.taurus/skills
```

GitHub Copilot reads the same specification, so its skills already work in
Taurus. The whole cost is that row. It's the one origin whose two directories
have different names. A repository's Copilot customizations live in the
folder GitHub already reads, not a dotdir of Copilot's own. So the drawer tags
a project skill `.github` and a personal one `.copilot`.

A project skill shadows a personal one of the same name. Within either tier, a
`.taurus` skill shadows a borrowed one, so you can override a skill you didn't
write without editing it. The drawer tags each row with where it came from,
and a shadowed skill is logged, not silently dropped.

Only one line per skill goes into the system prompt: its `when_to_use` if it
has one, or a condensed `description` if not, as with every skill written for
another client. The procedure itself loads on demand through the `load_skill`
tool. That's what makes a fifty-skill library affordable on a model with an 8k
context window.

`when_to_use` is an optional Taurus field. It's worth writing for skills you
keep here. The specification's `description` does two jobs at once: what the
skill does, and when to use it. When the whole context is 8k, 200 characters
aimed at the decision beat 1024 aimed at a catalog listing.

Loading is lenient, because a skill you already have is more useful read than
refused. A name in the wrong case, a name that disagrees with its directory, a
value with an unquoted colon: each one is repaired or tolerated, and the skill
loads. The problem is reported on the skill's row in the drawer and by
`taurus skills check`. Only an empty description, or YAML that no quoting can
rescue, stops a skill loading.

Skills Taurus writes itself are held to the strict rules. A proposal that
would only load thanks to leniency is rejected, not written.

You can also run any skill directly as a slash command, like
`/speckit-specify add a dark mode toggle`, in the app and in `taurus run`. The
harness fills the skill's `$ARGUMENTS` placeholder with the rest of the line
and hands the model the procedure instead of the command. A skill with no
placeholder gets the text appended under a heading, so it isn't lost.
Sub-agents share the same `/` namespace. See
[Slash commands](#slash-commands).

Two frontmatter flags decide how a skill can be reached:

- `disable-model-invocation: true` keeps a skill out of the prompt catalog but
  leaves it runnable by name. Use it for procedures that should run when a
  person asks and not before.
- `user-invocable: false` does the reverse.

Both are optional, and both default to available.

A skill's `scripts/`, `references/`, and `assets/` are listed when the skill is
opened, and read only if the procedure calls for one. That's the third tier of
progressive disclosure. Scripts left in `scripts/` without being declared in
the frontmatter are picked up by extension, so a skill written for another
client is runnable, not just readable. For the same reason, read-only tools
may reach into the directories of loaded skills. Writes stay inside the
workspace.

The agent proposes new skills through `propose_skill`. Every proposal is
validated before it reaches a review card:

- kebab-case name
- non-empty trigger under 200 characters
- no near-duplicate of an existing skill
- no destructive script patterns

Nothing touches disk until you approve it. Approving reloads the catalog, so
the skill is usable in the session that wrote it.

**A new or edited skill is available on your next message.** The eight
directories are checked at the start of each turn and rescanned only when
something in them moved. Drop a skill into `.taurus/skills`, or have another
client install one into a shared directory, and your next message can use it
without a reload.

Coming back to the window rescans too. That's the case that matters most:
most skills arrive by writing one in an editor and switching back.
Opening the Skills drawer also rescans, so the drawer and the rail's count
never show the startup catalog.

The catalog is frozen *within* a turn, like the sub-agent roster and for the
same reason. A turn has to run against the library it started with, so it
can't see a skill saved while it's running. The check itself is a `stat` per
skill. It saves parsing every `SKILL.md` in the library, which is why it's
worth asking first.

Scripts declare a logical interpreter (`python3`, `node`, `bash`, …), resolved
per platform at load time. If it can't be found, the skill is marked degraded
and the model is told to follow the written steps instead. A Python-dependent
skill doesn't hard-fail on a Windows machine.

## Sub-agents

A turn can hand a self-contained job to a sub-agent. The sub-agent gets its
own conversation, its own context window, and a narrower set of tools. The
parent sees only the child's conclusion, so a search that reads thirty files
costs it one paragraph.

Three ship with the harness. The parent picks one by answering a question
about its own task before it delegates:

| Agent | Use it when | Scoped to |
| --- | --- | --- |
| `explorer` | The answer is in the code and only needs reading. | `read_file`, `list_dir`, `glob`, `grep`, `load_skill` |
| `worker` | You can dictate the edit exactly. | Whatever the main agent has |
| `coder` | Someone has to look at the code and decide. | The file tools, `grep`/`glob`, `run_command`, `load_skill` |

`coder` is the one that checks its own work. It's told to read around the
change before writing it, then build it or run the tests and report what it
ran. That's also why it's scoped to a named list instead of inheriting. An
agent that advertises "builds or tests it" but quietly also holds a web client
and your MCP servers is advertising something else. The model reaches any of
them through `spawn_subagent`.

`coder` and `worker` overlap at the edges, because real tasks do. What keeps
them apart is who decides. Hand `worker` a decision it wasn't given and it's
told to stop and say what's missing, not guess.

Two delegations in one round run side by side only if neither agent can
write. `explorer` can't, so two searches run together. `coder` and `worker`
can, so each runs on its own: two agents editing one working tree at once
would each write over files the other is halfway through. Your own agents are
judged by the tools they name. An agent with no `tools:` key inherits
everything the parent has, writers included, so it runs on its own too.

A delegate's tool calls meet the same `pre_tool_use` and `post_tool_use` hooks
as the parent's, so it can't route around a guard. It doesn't fire
`user_prompt_submit` or `stop`: a delegation is one call inside your turn,
not a turn of its own.

You can add your own. An agent is a markdown file in `~/.taurus/agents` or
`<workspace>/.taurus/agents`. The file name is the agent's name, and the body
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

Agents are read from six directories, the same shape the skill library
uses. The borrowed locations come first, Taurus's own last:

```
~/.claude/agents/<name>.md           <workspace>/.claude/agents/<name>.md
~/.copilot/agents/<name>.agent.md    <workspace>/.github/agents/<name>.agent.md
~/.taurus/agents/<name>.md           <workspace>/.taurus/agents/<name>.md
```

The first two rows are Claude's and GitHub Copilot's, read for the same reason
as `.claude/skills`. Their agents have the same shape as a Taurus agent:
frontmatter, plus a markdown body that's the system prompt. So reading them
costs directories, not a second parser.

Copilot's doubled extension is understood: `reviewer.agent.md` is the agent
`reviewer`, not one called `reviewer.agent`. Frontmatter keys Taurus doesn't
have are ignored, not honored.

A project agent shadows a personal one of the same name, and either shadows a
built-in. So an `explorer.md` of your own replaces the shipped explorer
instead of sitting beside it. Within a tier, yours wins over a borrowed one.
That's how you override an agent you didn't write without editing it. The
drawer says so on the row when that happens.

**A borrowed file is read and never written.** If you retune `max_iterations`
on a Copilot agent, Taurus saves its own copy beside it that shadows the
original, exactly as editing a built-in does. The field tells you this before
you touch it.

That isn't tidiness. The file is usually committed, Copilot still reads it,
and its frontmatter carries keys Taurus has never heard of. Taurus rewrites a
file from the fields it knows, so editing one in place would silently delete
every `handoffs:` and `hooks:` in it.

The copy lands in the same tier as the file it overrides. A user-tier copy of
a project-tier agent would sit underneath the thing it was meant to replace.

**A new or edited agent is available on your next message.** The directories
are checked at the start of each turn and rescanned only when something in
them moved. So you can write `reviewer.md` in an editor and delegate to it in
your next message, without a reload or a trip to the drawer.

The roster is still frozen *within* a turn, so a turn can't see a file saved
while it's running. That's deliberate. A turn has to delegate against the set
of agents it started with, and a file watcher couldn't promise that. The
drawer's **Rescan** is still there for when you want it now, not on the next
message.

The `/` command menu is the one surface that can't rescan on its own. It's
redrawn on every keystroke, and taking config locks there is how a reload
ends up deadlocking against your typing. It lists what the last scan found,
but it re-reads that list whenever the number of skills or agents changes. So
any rescan refreshes it: coming back to the window, finishing a message, or
opening either drawer. Typing the full name works straight away regardless,
because that path resolves against a fresh roster.

`max_iterations:` is how many model/tool round trips this agent gets before
it's stopped, between 1 and 100. You can edit it on the agent's card in the
Agents drawer as well as in the file. The card edit rewrites the file in
place, so `model:` and anything else you set by hand survives.

A built-in has no file to write, so editing its limit saves a copy you own
into `~/.taurus/agents` that shadows the built-in. The card says so before you
change it. The conversation that delegates has its own ceiling, a separate
number in Settings › Behavior. See
[the iteration ceiling](working-with-it.md#when-a-turn-stops).

`tools:` is **enforced**. It's exactly the set the child is offered. A skill's
`allowed_tools`, by contrast, is advisory. Leave the key out to inherit
everything the main agent has.

It narrows what the agent is *offered*, never what it's *permitted*. Every
call the child makes still meets the same permission gate as the parent's. If
every tool an agent names turns out to be unavailable here, the agent is
refused instead of run unscoped, because an empty list would otherwise mean
"everything".

`model:` runs one agent somewhere else: a bigger model for review, a smaller
one for search. If it names a provider that isn't configured on this machine,
the agent is degraded instead of failing to load. It runs on the session's
model, and both the drawer and `taurus agents check` say so. That way a repo
can ship an agent naming a cloud model without breaking for a contributor who
only runs Ollama.

A sub-agent can't delegate further. Its registry has no `spawn_subagent` in
it, so the depth cap is structural, not a counter the model could talk its way
past.

Every delegate keeps its own transcript, written as it runs, in a directory
named for the conversation that spawned it:

```text
~/.taurus/sessions/<workspace>/<id>.jsonl                    the conversation
~/.taurus/sessions/<workspace>/<id>/subagents/agent-*.jsonl  what it delegated
```

The parent's transcript still records a delegation as what it is: one call,
one paragraph back. The reading, the dead ends and the reasoning behind that
paragraph stay somewhere you can find them. A delegate isn't a conversation
somebody had, so it never appears in the session list. Deleting a
conversation deletes its delegates with it.

In the app, the delegation's row opens it in a drawer beside the
conversation. That works while the call is running, which is when a stuck
delegation is worth checking, and afterwards, when the paragraph it returned
is thinner than you expected. It opens read-only. A delegate's conversation
happened inside somebody else's turn, so there's nothing there to continue.

```bash
taurus agents list          # the roster, what each is scoped to, what it costs
taurus agents check         # non-zero if an agent will not load or cannot run as written
taurus sessions --agents ID # what one conversation delegated, and where it was written
```

You write agents in a text editor, as you do skills. The drawer's
**New agent…** writes a starter file with every key documented in place and
opens it. The drawer rescans on open, so editing a file and reopening shows
what's actually on disk.

The agent can also write one for you. `propose_agent` is the twin of
`propose_skill`, gated by its own setting. A skill is a procedure the model
follows, and an agent is a worker it hands a task to. Wanting one is no reason
to want the other.

A proposal is validated before it reaches a review card:

- kebab-case name
- a description under 200 characters
- a system prompt long enough to be worth a file
- no near-duplicate of an agent already on the roster
- no tool this session doesn't have

Nothing touches disk until you approve it. The card is editable, so it's
validated again on the way out, because a hand-edited name or tool list hasn't
been checked.

The risk stays bounded because a proposed agent can't reach past the session
that wrote it:

- `tools:` only ever narrows.
- A name outside the session's registry is refused, not saved and degraded.
- Every call the child makes still meets the parent's permission gate.
- The child has no `spawn_subagent`, so it can't propose or spawn more agents.

`model:` and `provider:` aren't proposable at all. Which model a delegate runs
on is a cost decision on a provider you pay for, and it's the one field with
no bearing on what the agent can do. An approved agent inherits the session's
model. To change it, edit the file, where the decision is yours and visible.

Approving rescans the roster instead of reloading everything, so saving an
agent doesn't restart every MCP server. Saving a server works the same way. It
reconnects the servers and rescans the roster only when the set of tools
those servers offer actually changed, which is the one way a server can
affect an agent.

A new agent isn't usable in the turn that proposed it, because a turn's roster
is frozen when it starts. The tool result tells the model, so it doesn't spend
a round trip finding out.

## Slash commands

One `/` namespace covers both libraries, in the app and in `taurus run` alike:

```
/speckit-specify add a dark mode toggle    # runs that skill's procedure
/reviewer check the auth module            # hands the job to that sub-agent
```

The composer completes as you type `/` and tags each row **skill** or
**agent**, because the two do different things with the rest of the line:

- A skill's procedure replaces your message.
- An agent's name becomes an instruction to delegate. The turn calls
  `spawn_subagent` with the line as the task, and what comes back is the
  child's conclusion, not the thirty files it read.

Delegation stays a tool call, not a separate code path. So a command runs
exactly the agent the model would have run on its own, with the same tool
scoping, the same permission gate, and the same depth cap.

`/explorer` with nothing after it points the agent at what the conversation
has already established. That's what "now do that part with the explorer"
means. Both built-ins are reachable this way on a machine with no agents
directory.

If a skill and an agent share a name, the command runs the skill. That isn't a
judgment about which is more useful. A command that quietly starts doing
something else is worse than a name that's awkward to reach. Rename one of
the two if you want both. A model-only skill (`user-invocable: false`) doesn't
reserve its name, so an agent behind one is still reachable.

If nothing matches a name, you get told instead of it being sent, along with
the near misses from both rosters. Turning `spawn_subagent` off in
`disabled_tools` takes agents out of the menu. Typing one anyway tells you
that, not "no such command".

Ordinary text that starts with a slash is never treated as a command, so
`/usr/bin/env is portable` is sent as written. A command has to name a skill
or an agent, start with a letter, and be followed by a space or nothing.

## The canvas

Ask to see a file and it opens in an editor beside the conversation:

> open the readme

> show me where the retry logic is in `crates/taurus-host/src/host.rs`

The second one opens on the passage, not at the top. The model passes the
lines it means, and the editor scrolls there and selects them. It's the
difference between being handed a file and being pointed at something in it.

The transcript keeps a card for every file that was opened. Click one to open
it again. The card holds a path and nothing else, so a conversation from last
month opens today's version of the file. That's the version worth looking at,
because you usually go back to check whether what was said is still true.

### It is a split, not a screen

The conversation stays where it is. That's the whole point: you put a file on
screen to talk about it while it's there. Drag the edge to give either side
more room. Close it with the ✕ and the conversation takes the width back.

Markdown opens on its rendered preview, with a **Source** switch for the
asterisks. Source files open as source, colored by the same tokenizer the
transcript uses.

### Asking about a passage

Select some text and **Ask about this** appears. It puts the start of a
sentence in the message box, `About lines 40–58 of host.rs: `, and leaves the
rest to you. Nothing is sent until you send it.

The selection goes with that message. The model is told which file was open,
which lines were highlighted, and what they said. On screen, "tighten this"
and "does this handle the empty case?" are complete questions. The selection
keeps them complete for the model, where a bare transcript wouldn't. Without a
selection, the model is told the file is open and to read it before answering
anything about its contents.

The chip above the message box shows this. It names the file and the lines
while you type, because context you can't see is behavior you can't explain.

### Typing in it

The editor takes edits and saves them itself about a second after you stop
typing. There's no ⌘S and nothing to remember. That's not a convenience. The
whole argument for the canvas is that you and Taurus are looking at the same
file, and an unsaved buffer breaks that silently. You'd ask about the
paragraph on screen and get an answer about the one on disk.

The header says **Unsaved** while a save is pending and **Saving…** while it
runs. Silence means it's written.

### When you both write at once

Taurus edits files too, and neither of you waits for the other.

- If Taurus writes the file you have open and you haven't typed anything, the
  editor takes the new version and tints what changed for a second. You watch
  it edit your document.
- If you *have* typed something, nothing is taken. A bar appears saying the
  file changed while you were typing, with **Keep mine** and **Take theirs**.
  Your version stays in the editor either way.

A save never overwrites something it hasn't seen. The editor holds the
fingerprint of the file as it read it, and a save that doesn't match is
refused. So the race is closed at the write, not papered over in the UI. While
a conflict is open, Taurus is told the file on disk isn't what's on screen, so
it can't answer confidently from the wrong version.

CRLF files stay CRLF. A browser reports its editor's contents with LF endings
whatever went in. Without this, a three-line edit would land as a diff
touching every line in the file.

### What it does not do yet

One file at a time: opening another replaces it. It opens text (source,
Markdown, config) up to 4 MB. There's no folding, no in-editor find, and one
cursor. A file changed outside a turn, like a `git checkout` in the dock, is
noticed when you save, not when it happens. See `docs/known-gaps.md`.

## Notes

Markdown files you write yourself, and sketches beside them, in two notebooks
with a pane of their own.

```
.taurus/notes/           project notes, committed with the repository
~/.taurus/notes/         global notes, yours across every project
```

A note is a file and nothing else: no frontmatter, no index, no format only
this app reads. The name in the list is the filename, so the two can't drift.
A note you add by hand in an editor shows up in the pane with no import step.

Nothing merges between the notebooks. Two notes with the same name in the two
scopes are two notes. Config merges, with the workspace layer winning, but
there's no sensible way to merge prose.

**Write** is the source and **Read** is what it renders as. That includes a
```` ```mermaid ```` fence, which the app's own diagram engines draw instead
of the Mermaid library. Editing saves itself a moment after you stop typing
and never overwrites a version it hasn't seen. If a turn writes the same note
while you're typing in it, both versions stay on screen and neither is
chosen. That's the canvas's rule, with the canvas's implementation.

A **sketch** is an Excalidraw drawing: a `.excalidraw` file in the same
notebook, under the same rules, opened full-pane in the editor. A note draws
one in Read with a Markdown image line, `![caption](<Name.excalidraw>)`. That's
also what **Copy embed** gives you.

The model reaches a note with `read_note`, by notebook and name, not by path.
A global note is outside the workspace, and `read_file` won't go above the
root. Nothing else is reachable through `read_note`. The model has two more
tools:

- `write_note` writes a note. It's a write like any other: asked first, with
  the diff, and rewindable for a project note.
- `open_note` shows you a note as a card you open yourself.

The model can't see a sketch. What reaches it is the text written on the
sketches a note embeds.

These are separate from [Memory](#memory) above, and the difference is worth
keeping straight:

- Memory is written by the model, capped at a couple of sentences, and read
  into every later conversation's prompt.
- A note is written by you, is as long as you like, and costs nothing until
  you ask about it.

## Terminal

<kbd>Ctrl</kbd>+<kbd>`</kbd> opens a shell at the bottom of the window, in the
folder the window is pointed at. It's your own shell (`$SHELL` on macOS and
Linux), started the way any other terminal starts one. So it reads the rc file
you already have. Your aliases, prompt and completions are all there, because
none of it is reimplemented here.

On Windows it opens PowerShell, the way Windows Terminal does:

- PowerShell 7 if you have it
- otherwise the Windows PowerShell that ships in the box
- `cmd.exe` only where neither is on the PATH

The banner is suppressed. That's the one difference from Windows Terminal,
which has a full screen to spend on three lines of copyright. This dock
doesn't.

There's a real pseudo-terminal underneath and a real emulator on top.
Together they make it a terminal and not a log with colors in it. `vim`
opens. `htop` redraws. `less` pages. A progress bar overwrites its own line
instead of printing a hundred of them. Resizing the pane tells the shell its
new geometry, so a full-screen program reflows with it instead of drawing at
the size it started at.

### The commands the model started

![The terminal dock with three tabs — the shell, a `cargo test --workspace`
that exited 101, and a `pnpm dev --host` still running — showing the failing
test's output](screenshots/background.png)

A background command gets a tab beside the shell. `run_command` with
`background: true` hands the model a number and keeps printing into a buffer
between turns. By the time anything arrives, the card for the call that
started it is closed. Without the tab, the model could read a build you
couldn't see.

The tab strip only appears when there's a background command to show. The
rail's **Terminal** row shows a count while any are running, so you find out
about a build started behind a closed dock.

Each tab shows:

- the command
- how it's doing, in the same words `check_command` gives the model
- a **Stop** button while it's still going

A finished one is marked as well as colored: `✓` clean, `✗` a non-zero exit,
`–` stopped. So the strip reads on a projector, and for someone who can't tell
the two colors apart.

These tabs are read-only by design, not a half-built feature. A background
command runs with pipes and no pseudo-terminal, so there's nothing to type
into and nothing addressing a screen by coordinate. What it printed is text,
wrapped instead of scrolled sideways. Escape sequences from a command that
colors anyway are stripped on the way in. The pane follows the output down
until you scroll up, and picks the tail back up when you scroll to the bottom.

The pane and the model read the same buffer but keep separate places in it.
Opening a tab doesn't take lines out of the model's next `check_command`, and
a check doesn't blank the tab. See
[Commands that keep running](safety.md#commands-that-keep-running). Anything
the buffer has dropped is gone from both. The pane marks where with a
bracketed line at the actual gap, not a note off to one side.

The dock is desktop-only on purpose. `taurus` on the command line is already
in a terminal, and a second one inside it would be a worse version of the one
it's running in.

Closing the dock ends the shell, the same as closing a terminal window
anywhere else. So does closing the app. That's worth stating, because a shell
started under a pty isn't in the app's process tree in any way the operating
system would clean up on its own.

What it is **not**, yet: the shell isn't wired to the conversation.

- The agent doesn't read what you type there.
- What you run isn't checkpointed the way the agent's own commands are.
- Commands are a scrollback, not the blocks a Warp-style terminal groups them
  into.

Each of those is written down in [Known gaps](known-gaps.md).
