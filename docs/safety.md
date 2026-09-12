# Permission, and undo

<sub>[← Taurus AI Shell](../README.md)</sub>

None of this is a confirmation dialog for its own sake. An agent that edits
files and runs commands leaves you with two questions: what is it about to do,
and how do you put it back? Everything here answers one of them.

## Permissions

Read-only tools inside the workspace run unattended. Writes, command execution,
and network access prompt with the exact call. Shell approvals are keyed by the
leading command word, so approving `git` doesn't also approve `rm`. A call that
names a URL is keyed the same way, by that URL's host. Approving `fetch_url`
for `docs.rs` is a decision about one site, not a standing grant to reach
anywhere.

**A write is shown as a diff.** `Write src/widget.rs (2140 bytes)` tells you a
file is about to be replaced, but not with what. For a new file that's enough.
For an overwrite it's the least informative moment in the product: the bytes
being destroyed are on disk, and the bytes replacing them are in the tool call.
So Taurus diffs them, in the desktop dialog and on the terminal. The diff uses
the same read the checkpoint log takes a moment later, so it costs a read that
was going to happen anyway.

![The permission dialog showing a diff of the change a write would
make](screenshots/permission-diff.png)

A line diff on its own tells you a line was replaced and leaves you to find the
difference. Two things narrow that down:

- The text is coloured by the file's own language.
- Where a removal is answered by exactly one addition (the same line before
  and after), the characters that actually differ are marked inside them.

Above, `MAX_ITERATIONS` and `self.config.max_iterations` carry the mark. The
line below them was added, not rewritten, and answers nothing, so nothing in it
is marked. See
[Code is coloured, and so is a diff](working-with-it.md#code-is-coloured-and-so-is-a-diff)
for the two cases where it declines to guess. Either way, the `+` and `−` in
the gutter stay the primary signal.

The diff `edit_file` shows comes from running the call's own replacement,
through the same function the tool uses. A dialog that shows one change while
the tool makes another is worse than no diff at all, because it's what you
believed when you approved it.

Both diffs are capped at 160 lines and say how many they left out. A wall of
lines in a modal gets approved unread. That's the same failure as a dialog
showing the wrong change, reached from the other side. A write that would
change nothing says so instead of showing an empty frame. That's usually a
model looping, and it's a decision worth not making. A file that isn't text
gets no diff at all, because a diff of replacement characters tells you no
more than the byte count did.

The two narrow answers each have a key, printed on the button: ⌘↵ for **Allow
once** and ⌘⌫ for **Deny** (Ctrl on Windows and Linux). This is the
most-pressed control in the app. A turn that greps, reads, edits and runs the
tests stops here four times. A session that needs the mouse four times a turn
isn't keyboard-driven.

They're chords, not bare Enter and bare Escape, and that's a safety choice, not
a style one. The dialog appears under whatever you're typing, and nothing in it
has focus when it opens. A prompt that opened with the affirmative focused
would turn a stray Enter into a grant nobody read. A chord isn't a stray
keystroke.

**Neither standing grant has a key.** "Always" is a decision about every
future call, and that deserves a deliberate press.

An "allow always" decision persists into one of two layers, and Taurus
consults both:

| | Persists to | Asked again in a new workspace |
| --- | --- | --- |
| **Always here** | `<workspace>/.taurus/permissions.json` | Yes |
| **Always everywhere** | `~/.taurus/permissions.json` | No |

**Always here is not offered in an untrusted workspace.** Taurus isn't reading
that file there, so a standing decision would have nowhere to live. See
[Trusting a workspace](#trusting-a-workspace). The call itself is still
allowed. Only the standing part of your answer is dropped.

**Everywhere is not offered for running commands.** A workspace grant for `git`
is scoped to a project you've already decided to trust. The same grant made
globally applies in every repository you ever open, including one you just
cloned to look at. That's the decision most worth making per project.

The restriction covers what Taurus will *create*. A `run_command:*` rule you
write into the global file by hand is still honoured. Editing that file is an
explicit act, and silently ignoring it would be its own surprise.

**A command that does more than run one program is never covered by a
standing grant.** A grant for `git` is keyed by the command's first word, and
the first word says nothing about the rest of the line: `git status; rm -rf ~`
starts with `git`. So Taurus asks every time, and never offers "always", for a
command that:

- chains another onto it (`;`, `&&`, `||`, `&`, a new line)
- pipes into another program
- runs a command inside it (`$(…)`, backticks, `<(…)`)
- writes its output into a file (`>`)
- starts with a word that only hands the rest of the line to another program:
  `sh -c`, `env`, `xargs`, `sudo`, `nohup`, and the like

A grant you've already saved doesn't cover one either. Two things don't count:
merging one of the program's own streams into another (`2>&1`), and throwing
output away (`2>/dev/null`).

The check doesn't parse shell, so a `;` inside quotes counts too, and
`git commit -m "fix; tidy"` is asked about each time. A check that parses shell
wrongly is worse than one that asks.

Every path argument is canonicalized and checked against the workspace root.
That closes both `../` traversal and symlink escapes. A symlink that doesn't
resolve (its target is missing, or it loops) is refused, not guessed at. A
write through it would create the target wherever the link points, inside the
workspace or not.

**With no terminal** (a pipe, a git hook, CI) there's nobody to prompt, and
both obvious defaults are wrong. Allowing everything hands an unattended model
the machine. Denying everything is useless without saying why. So the CLI
takes an explicit policy, and when it refuses something it names the flag that
would have permitted it:

```
$ taurus run "summarize the readme into SUMMARY.md" < /dev/null
  refused (write): Write SUMMARY.md (214 bytes)
    no terminal to ask; re-run with --allow write_file to permit it

$ taurus run --allow write_file "summarize the readme into SUMMARY.md"
```

`--allow-command git` grants a shell program by its leading word. That's the
same unit the interactive "allow always" uses, with the same limit: a command
that does more than run that program isn't covered, and the refusal says why.
`--dangerously-allow-all` exists for throwaway or already-sandboxed
environments.

Skills are never saved unattended. If the agent proposes one during a piped
run, the CLI reports it and discards it, so nothing gets written that nobody
reviewed.

## Trusting a workspace

Everything above covers decisions Taurus asks you to make. Trust covers the
decisions a folder would make for you.

A workspace's own `.taurus` isn't passive data:

- `mcp.json` starts child processes.
- `providers.json` names the endpoint every message of every conversation is
  sent to.
- `search.json` decides whether `fetch_url` may reach private hosts.
- A skill can carry a script.
- `permissions.json` is a standing grant. It's the one file in a repository
  that hands over capability with no prompt at all:
  `{"allowed": ["run_command:rm"]}` in a clone is an "always allow" nobody ever
  clicked.

All of that arrives with `git clone`, so none of it takes effect until you say
so:

**An untrusted workspace contributes no config at all.** There's no per-file
carve-out, just one rule in one direction. Your own `~/.taurus` applies in
full, so Taurus works normally in a folder you haven't vouched for. It just
won't take instructions from it.

Because the rule is that blunt, it's easy to check. Every project-tier read in
the harness resolves the workspace through a single function. A read that
skipped it would get `None`: the state that already exists at startup, before
you've chosen a workspace, and one every loader already handles.

**You are only asked when there is something to answer.** A folder with no
config of its own never raises the question, and that's most folders. When the
question does appear, it names what's waiting, and it shows the MCP command
lines instead of counting them:

```
This project has configuration Taurus is not reading.
  1 skill
  1 MCP server
      probe: npx -y some-package
  2 standing permission grants — tools this project would allow without asking
```

Nobody can judge a count of servers. You can judge `npx -y some-package`.

The same argument applies to the rest of the layer. A skill is a file with a
procedure in it. An `AGENTS.md` is a brief the model gets before it reads a
line of the code. So Taurus reads those files, and names anything in them worth
your eyes beside the count:

```
Worth reading first:
  AGENTS.md — invisible characters
    1 character that renders as nothing or reorders the text around it, first
    on line 3. The model is given this file as text and reads what you cannot
    see.
  .taurus/skills/deploy — a skill carrying a script
    carries setup.sh — read it before trusting this workspace
  .taurus/providers.json — names an endpoint
    every message would be sent to gateway.example.com
```

It looks for seven things:

- characters that render as nothing or reorder what is around them
- HTML comments long enough to hold an instruction
- runs of base64 long enough to be a program
- the endpoint `providers.json` names
- `search.json` lifting the guard that keeps `fetch_url` off private hosts
- standing grants wide enough to cover a whole tool
- a skill carrying something executable

Most of these are things a reviewer couldn't have seen by reading the file.

**None of it is a verdict.** A workspace with nothing found isn't a workspace
that's safe to run. This reads configuration, not behaviour. And the two rules
most likely to matter are the ones written to *stay quiet*:

- A zero-width joiner is how a family emoji is built, and it's never flagged.
- An image embedded in Markdown is a long run of base64, and it's never
  flagged either.

A scanner that fires on ordinary repositories is one people learn to click
past, and that would cost you exactly the workspace where it mattered.

`cargo run -p taurus-host --example inspect -- .` runs the same rules over any
folder without starting the app. Use it to check that claim on your own disk.

The desktop app shows this in a banner above the composer, not a modal on open.
Nothing from the folder is loaded, so nothing is waiting on your answer. And a
modal you have to clear before starting work is how a security prompt turns
into a reflex. The terminal prints one line per command and answers with
`taurus trust --allow`. See
[Trusting a workspace](configuration.md#trusting-a-workspace) for the commands.

Declining records nothing. A workspace you waved off and one you've never
opened are the same state on disk. That's what lets a `git pull` that adds a
server ask again.

## Running commands

Commands run with three pipes and no stdin by default. That's right for almost
everything an agent runs. A model can't answer a `[y/N]` prompt, so a command
that waits for one must fail on the timeout instead of hanging the session.

The exception is a program that checks whether it's talking to a terminal. Told
no, `git` pages and colours nothing, `npm create` declines to scaffold, and
anything built on a full-screen prompt library fails at startup. A person would
never see that behaviour and couldn't easily explain it. These aren't exotic
commands. They're the ones somebody would reach for.

So `run_command` takes two more arguments:

- **`pty: true`** runs the command under a real pseudo-terminal: `forkpty` on
  Unix, ConPTY on Windows, from the one codebase. The program believes it's on
  a terminal because it is.
- **`stdin`** hands it the keystrokes up front. A pty isn't the same as
  interactivity. Under one, a program that wants an answer still waits for it,
  so the two arrive together. This is what turns "behaves correctly" into
  "completes", and it works on the piped path too.

A terminal has one stream, so under a pty stdout and stderr are interleaved,
and the `[stderr]` split the model reads elsewhere can't be recovered. That's a
property of terminals, not of this implementation.

Terminal control sequences are stripped before the output reaches the model. A
`cargo build` under a pty is more escape bytes than text, and the model pays
tokens for every one of them. Bare carriage returns go too, so a progress bar
doesn't produce a transcript that scrolls over itself.

The timeout still holds, and it has to. Under a pty an interactive program
waits instead of hitting end-of-file, so a ceiling that didn't fire would hang
the session for good. The timeout kills the child instead of abandoning it. A
blocking read can't be cancelled, and a worker parked on a child that never
exits would otherwise outlive the session that started it.

**A machine with no usable pty still runs the command.** Asking for a terminal
and not getting one isn't the command's fault, so it runs with ordinary pipes
instead of failing. The result says so, above the output, not below it. That
order matters. A program told it's not on a terminal pages nothing and colours
nothing, which reads like a fact about the project unless you already know the
terminal never arrived.

**On Windows the pty is a ConPTY, and a ConPTY is a real console host.** The
system's host shows a window when the process asking for it has none. A
release build of the desktop app has none, so every `pty: true` command would
open a console window for as long as it ran.

So the bundle ships Microsoft's redistributable ConPTY beside the executable.
Its host is headless, and `portable-pty` prefers it over the system's. It costs
2.2 MB in the Windows installer: one console host for x64 and a second for
ARM64. The second is there because an x64 build runs on ARM64 Windows under
emulation, and the host there has to be native.

Neither the CLI nor a development build is affected, because both already have
a console. That's why this only ever shows up in the installed app. See
[`scripts/conpty.mjs`](../scripts/conpty.mjs).

### Output too big to hand over

A waited-for command's output is capped at a share of the model's context
window: 64 KB per stream on a 200,000-token model, 4 KB on an 8k local one,
320 KB on a million-token one. Past that, the head and the tail are kept and
the middle is dropped. The tail matters as much as the head, because a
compiler's verdict and a test runner's count are the last thing they print. A
cut that kept only the beginning would take exactly the line worth reading.

**A test run keeps its failures and counts its passes.** A test suite is the
output an agent runs into most, and it's the worst possible shape for a byte
count: thousands of lines saying nothing happened, around the handful saying
something did. Cutting the middle out would take the failures and keep the
passes.

So the lines that say a test passed are replaced by
`[… 404 passing tests not shown …]`, and everything else survives as it
arrived: the compiler's warnings, the `failures:` block with its panics, the
counts at the end, the ignored tests. On this project's own
`cargo test -p taurus-tools --lib`, 30,784 bytes become 312.

**A warning already shown in full is not shown again.** A Rust diagnostic is a
headline, a location, the source it's about, and a suggestion. That's eight
lines for one warning, and a rename that touches two hundred call sites prints
those eight lines two hundred times. A repeat keeps its headline and location
and loses the rest to `[… body not repeated; the first is above …]`. So every
warning still appears and still says where it is.

Errors are never touched. They're what the command was run to find out, there
are rarely many, and an abbreviated error is worth less than the full one.
Measured against a real `cargo clippy` run, this takes off 45%.

Grouping warnings by lint name would be better, but the output doesn't allow
it. clippy names its lint in a `help:` line every time. rustc names its own in
a `#[warn(...)]` note it prints only on the first occurrence, so half the
warnings in a build carry nothing to group by. Every block has a headline, so
the headline is what gets matched.

Taurus picks the filter by reading the output, not by parsing the command line.
`cargo test` behind a pipeline, a binary in `target/debug/deps` run directly,
and a log being `cat`ed all produce the same output, and none of them says
"cargo test" anywhere worth matching. libtest announces itself with
`running 404 tests`, and nothing outside that announcement is touched.

**Repetition is collapsed before any of that happens**, so usually the cut
never has to. This applies to a stream large enough to be worth the pass: 16 KB
on a 200,000-token model, and a share of the window like every other cap here.
When such a stream says the same thing three or more times in a row, it keeps
one copy and a sentence: `[… the line above repeated 3999 more times …]`. A dev
server retrying a connection four thousand times is a hundred kilobytes that a
reader would call one line, and a byte count can't tell the difference.

Nothing is paraphrased. A line either survives as itself or is replaced by a
sentence saying exactly what stood there. That's what lets a model tell a
shortened stream from an untrustworthy one. Under that size nothing is touched
at all, which covers almost every command an agent runs.

**What was dropped is still somewhere.** The whole stream is written to
`~/.taurus/output/<workspace-key>/`, and the gap in what the model reads names
the file and says how to open it. That's the difference between a cut and a
loss. Without the path, the only way back to the middle of a four-minute build
is to run the build again: minutes, and a second set of side effects, to look
at something that already happened.

The log's size doesn't decide whether it can be read. `read_file` takes its
window around the offset it's given, not from the first byte, so line 40,000 of
a ten-megabyte build opens for what the window costs. See
[The context window](working-with-it.md#the-context-window).

The log goes in the config home, not the project, for the same reason
transcripts and checkpoints do. A build log holds the contents of the files it
compiled, and in the repository it's something somebody commits by accident.

The directory is readable and nothing more. It's added to the same read-only
list that lets a skill open its own bundled files, so the model can read a
stream back and still can't write anywhere outside the workspace. Twenty
streams are kept per workspace, and the oldest go as new ones arrive.

Where there's nowhere to write (a piped CLI run, a tool called directly), the
gap says how much it dropped and nothing more. A copy that can't be made isn't
a reason to fail a command that ran.

### Commands that keep running

A build from cold, a whole test suite, and a dev server are three things the
paragraphs above can't handle. The timeout is ten minutes at the outside, and a
turn spent waiting is a turn doing nothing else. So `run_command` takes a third
argument, **`background: true`**. It starts the command and comes straight back
with a number for it.

- `check_command` reads what the command has said since the last check, and
  can wait for it to finish.
- `stop_command` ends it, and answers only once it's gone. A kill that hasn't
  landed ten seconds later comes back as a command still running, not as a
  stop.

Eight may run at once.

Output arrives once *per reader*. Nothing holds the pipes open on the model's
behalf, so a background command's output is drained into a buffer as it
arrives. A check reads whatever has landed since the last one.

The buffer holds 256 KB, and one check hands over at most 64 KB of it. Past
either limit the oldest output is dropped and *counted*. So a check that
arrives too late learns that it did, instead of reading a prefix as though it
were everything. Both streams go into the one buffer in the order they arrived,
which is what a terminal would have shown. The `[stderr]` split a waited-for
command gets isn't available here.

The window is the second reader, and it keeps its own place in that buffer. See
[Terminal](capabilities.md#terminal). Neither takes lines from the other: a tab
drawing a build doesn't empty what the next `check_command` was going to read,
and a check doesn't blank the pane.

**What it changed is still recorded, and from before it ran.** The sweep that
makes a command undoable reads the workspace before the command starts and
again when it finishes ([Rewinding a turn](#rewinding-a-turn)). For a
background command those reads are minutes apart, usually in different turns.
So the command carries its own pre-image from the moment it started, and
compares against it the moment the process exits.

The changes land in the turn running at the next tool call after the command
finished, not the turn that started it, which by then is history. They land
whether or not anybody checked: every tool call collects the commands that
have finished since the last one.

Files other calls changed while it ran are left to their own turns, so undoing
a dev server's turn never undoes the edits made while it served. A command
still running when a turn ends isn't in any turn's changed-file list yet.
That's the honest answer, because it hasn't finished changing them.

**Two arguments are refused rather than ignored.** `pty` has nothing watching
the terminal it would open, and `timeout_secs` has nothing waiting to enforce
it. Both answer a question the caller asked. Quietly not honouring one is how a
model concludes the wrong thing about what it just started.

**They end with the workspace and with the window.** Nothing in the operating
system tidies up a child that outlived the call that spawned it. So switching
workspaces stops them, because their `cwd` is about to stop meaning what it
meant. Closing the window stops them too.

Switching also *forgets* them. Otherwise the roster and the dock's tabs would
name commands that ran in the old folder, each still holding a picture of it
to sweep against. A `taurus` CLI run ends them when the process ends. Between
turns, and across conversations in the same workspace, they keep running.
That's what they're for.

**The terminal dock is not on this path at all.** Everything above is about
commands the *model* asked for. The permission engine sees them, a rule can
refuse them, and a sweep records what they changed so a rewind can undo it.

A command you type in the terminal dock goes straight to your shell. Nothing
prompts, because nothing should: you're the one at the keyboard, and a
terminal that asked permission before each line would be a worse terminal. But
the consequence belongs here with the rest. What you run there isn't
checkpointed, isn't recorded in the Changes panel, and can't be undone by a
rewind. The two halves of the window make different promises. See
[Terminal](capabilities.md#terminal).

## Rewinding a turn

A transcript remembers that the model called `edit_file`. It doesn't remember
the bytes that were there first. So if a model rewrites the wrong file, or gets
an edit subtly wrong across a dozen call sites, nothing else in the harness can
give that work back.

So Taurus keeps the bytes. Before a tool changes a file, the file's current
contents go into an append-only log beside the transcript, and you can undo any
turn:

```bash
taurus rewind                        # turns that changed files, newest first
taurus rewind --to last --dry-run    # exactly what undoing the last one does
taurus rewind --to 3                 # back to just before turn 3
```

The desktop app's **Changes** panel is the same thing with a button. It docks
beside the conversation, not over it, and that's the whole reason it's worth
having as a panel. You answer "why is this line here" by scrolling up in the
transcript. A view you had to close to do that would be asking you to hold a
diff in your head.

It shares the column the canvas uses, so opening it hides an open file.
Shutting it brings that file back with its unsaved typing, because the file was
hidden, not closed.

![The Changes panel beside the conversation, unfolded to one diff per file
across the whole conversation](screenshots/changes.png)

There are two views of the same log, and they answer different questions:

- The turn list is what changed and when, one line each, with the rewind and
  the commit hanging off it.
- **View the whole diff** answers the question you ask before a commit: what
  these files hold now against what they held when the conversation started.
  It isn't the per-turn diffs added up. A file edited in four turns has four of
  those and one of these. It's folded shut because unfolding it reads every
  file the conversation touched.

Rewinding to turn *N* undoes every turn from *N* onward, not only that one. The
log records what a file held before a turn, and restoring one turn while
leaving a later one in place would produce a tree that never existed. Where two
turns touched the same file, the oldest pre-image wins, because it predates all
of them.

Both frontends show the plan before they write, and neither will rewind
unattended. A rewind discards whatever is in those files *now*, including edits
you've made by hand since. Piped, it names the flag that would have allowed it,
the same way a refused tool call does:

```
$ taurus rewind --to last < /dev/null
  reverted  src/widget.rs
  deleted   src/widget_test.rs
taurus: no terminal to confirm on; re-run with --yes to rewind, or --dry-run to
        see the plan
```

A file that wasn't text when it was recorded is reported as `skipped`, not
silently left as the model made it. `taurus rewind` exits non-zero when
anything couldn't be put back.

### Commands are covered too

A tool that can name what it will change declares it, and the log reads the
file just before the call. `run_command` can't name anything. A command line
doesn't say which files it will rewrite, and a guess would be worse than no
answer.

So Taurus doesn't ask. It indexes the workspace before the command runs and
walks it again when it finishes, and the difference is the answer. Anything
whose length or modification time moved, or that appeared or vanished, is a
change, and its contents from the first pass become its pre-image. A rewind
then treats it exactly like an `edit_file`.

`sed -i` across a dozen files, an `rm` that took the wrong directory, a script
the model wrote and ran: all of it comes back. All of it also appears in the
changed-file count and the **Changes** drawer, which is where you look to
decide whether you want it back.

This runs whether the command succeeded, failed, timed out, or was cancelled.
A command killed halfway through has still written whatever it got as far as
writing, and that's exactly the turn you want to undo.

The walk covers the workspace minus `.git` and minus `.taurus`, and it draws
one more line. A directory `.gitignore` excludes isn't entered, but a file it
excludes by name is still covered. Why:

- Indexing `target/` and `node_modules/` would cost gigabytes on every command,
  and a rewind that deleted build output would be a worse surprise than one
  that leaves it alone.
- The file an ignore rule usually names is `.env`, and a command that clobbers
  *that* is exactly the one you want back.

The split comes from the walk itself, not from a list of blessed names. Every
directory the walk enters is also read flat, which turns up the entries the
walk would skip. A directory it never enters is never read either. So `.env`
beside a `Cargo.toml` is covered, and `target/` beside it isn't. On this
repository the two passes cost about 21 ms and 8 ms.

Because `.env` is held, the checkpoint log holds it too, so those logs are
readable by their owner and nobody else. A file kept out of version control on
purpose shouldn't become world-readable because it was made recoverable.

**A command that moved git says so.** `.git` isn't swept and isn't restored.
So undoing a turn that ran `git checkout` or `git reset --hard` puts the files
back and leaves `HEAD` where the command left it, giving you a tree that
matches neither commit.

Restoring that properly would mean snapshotting the object store, which is its
own feature. Noticing costs two small reads, so the sweep reads `.git/HEAD` and
the branch it names before and after each command. When git's state moved and
files were recorded too, the result says why it isn't the whole story:

```
[taurus] This command moved git's own state as well. A rewind puts the files
back but leaves HEAD and the index where the command left them, so the result
would match neither commit; `git reflog` is the way back to where HEAD was.
```

It only appears when files were recorded. That's what keeps it a warning, not
a running commentary. A `git commit` that touched no working-tree file leaves
nothing to undo, so nothing looks undoable and there's nothing to correct.
Neither read covers `.git/index`. `git status` rewrites it to refresh its stat
cache, and a note on every turn that ran one would drown out the turns that
matter.

That message reaches the model while the command runs, which isn't when it's
needed. The person who needs it is reading a rewind plan, possibly days later
and certainly in another frame of mind. So it's written into the checkpoint log
as well as said, and comes back out at the moment of the undo. See below.

When a command *can't* be covered at all (a workspace past 50,000 files, or one
Taurus couldn't read before or after the command), the tool result says so in
plain words instead of letting the turn look undoable:

```
[taurus] This workspace holds more than 50000 files, too many to record a
command's changes against, so this one cannot be undone.
```

A command that rewrites an ignore rule is covered only in part. Once
`.gitignore` changes, a file the command created looks exactly like one it
just stopped ignoring. Recording that as new would let a rewind delete a file
that was there all along. So Taurus still records every file the command
modified or deleted, leaves out the files it created, and says so in the tool
result.

### What a rewind cannot put back

A rewind can only restore files, and three things routinely stop that from
getting you all the way back. None of them can be recovered here. All three
are knowable, though, so Taurus records them as they happen and reports them
beside the plan, before you press the button, not after:

```
Rewinding to before turn 4 undoes 2 turns in ~/src/parser:

  reverted  src/parse.rs
  reverted  src/lex.rs

  ! Turn 4 moved git's own state. Its files come back; HEAD and the index
    stay where the command left them, so the result will match neither
    commit. `git reflog` is the way back to where HEAD was.

  ! Turn 5 was committed as a1b2c3d. Undoing it leaves that commit in place,
    so the tree will no longer match it — `git revert a1b2c3d` undoes it as
    a new commit, `git reset` moves the branch off it.

  ! These turns ran on feat/parser, and the workspace is on main. Their
    pre-images came out of a tree that is no longer checked out, so a rewind
    writes them over main as it stands.

Overwrite 2 file(s) with what was there before? [y/N]:
```

The **Changes** drawer shows the same three, between the file list and the
button, and again afterwards. A commit left pointing at a tree that's gone
doesn't stop being a problem because the rewind finished.
`taurus rewind`'s own listing carries a shorter version, under the turns it
applies to:

```
  turn 2    tidy the caller
            src/main.rs
            committed as a1b2c3d · on feat/parser
  turn 1    teach the parser about tabs
            src/parse.rs
            moved git's own state · on feat/parser
```

These lines only appear when there's something to say. A conversation that
stayed on one branch and committed nothing is the common case, and a row of
empty fields under every turn would make the list harder to read, not more
complete.

A dry run produces them exactly as a real one does, which is the point of
having them. They're ordered by the turns they describe. A rewind of turns that
only changed files carries none, because a warning that fires every time is one
nobody reads.

The branch line compares what each turn recorded against what's checked out
now, so a conversation that never left its branch says nothing about it. A
detached `HEAD` and a workspace outside a repository both count as no branch,
not as a branch named something. Neither gives a name that would still mean
anything quoted back weeks later.

None of this refuses the rewind. It's your tree, and there are good reasons to
want the files back regardless. There's no good reason to find out afterwards.

## What a trace carries

Tracing is off until you name a collector. Naming one sends the *shape* of a
turn: which model, how many tokens in and out, how long each call took, which
tools ran, and which failed. That's a description of the work. It isn't the
work.

One thing is kept without a collector, and it's worth being exact about. Those
same spans go into a bounded ring in memory so the app's Traces panel can draw
them. The ring has no endpoint, no file, and no lifetime past the process. It
holds what the panel shows: a model name, a tool name, durations and token
counts. The two message fields aren't among its columns, so
`otlp_capture_content` can't put the conversation into it even when it's on.
Nothing in it leaves the machine, and quitting the app clears it if **Clear**
isn't to hand.

The conversation is a separate setting, `otlp_capture_content`, and it's off.
Turning it on puts `gen_ai.input.messages` and `gen_ai.output.messages` on
every completion span. That's the file the model read, the command it ran, the
diff it wrote, and whatever you pasted into the composer, sent to whatever
address is configured. There's no redaction pass and there isn't going to be
one. A harness can't know which line of a file it was handed is the secret.

Two things follow, and both are deliberate.

**Nothing infers content capture from an endpoint being set.** They're separate
decisions with different stakes, so they're separate switches. Turning
telemetry on tells you what a turn cost. It never tells anyone what was in it.

**The setting is read per turn, not at launch.** If you've just realized what
you switched on, you can switch it off and it takes effect on your next
message, not the next restart.

If you're pointing this at a collector you don't run yourself, leave content
capture alone. The token counts are the part that makes a dashboard useful, and
they're the part that can't leak a workspace.

## Keeping a turn

A rewind is the way back. This is the way forward, and it's the same list read
in the other direction.

The **Changes** drawer knows exactly which files each turn touched, and it
already holds what they looked like before. That's both halves of a diff, so
you can open up any turn and read it as one:

```
Turn 3 · 4m ago                                        2 files
  rename Widget to Gadget
  src/widget.rs · src/lib.rs

  [ Hide changes ]  [ Rewind to before this ]

  ┌ replace  src/widget.rs                             +12  −9
  │ 41  41    impl Widget {
  │ 42    -       pub fn new() -> Self {
  │     42 +      pub fn new(name: &str) -> Self {
  └ …
```

Nothing extra is written to produce those. A turn records what a file held
*before* it touched it. So the pre-image recorded by the next turn to touch
that file is, by construction, what this turn left behind. For the last turn to
touch it, what it left behind is still on disk. No post-images, no second
store.

The seam is a hand edit between two turns. It lands in the later turn's diff
instead of being attributed to nobody. That's the same assumption the rewind
already states when it warns it will overwrite "anything you changed by hand
since".

A file whose pre-image couldn't be held (not text, or too large) is named with
its reason, not left out. These are the same files a rewind reports as
`skipped`. A turn that looks smaller than it was would be the worst version of
this view.

### Reading it back to somebody who did not write it

**Review this turn** sits beside the diff, above the offer to keep it, because
reading a change over is what you do *before* deciding to keep it.

It hands that diff to an agent with none of this conversation in it: not the
request, not the plan, not what you said when you rejected the first attempt.
That's the whole idea, not a way to save tokens. An agent asked to check its
own turn has to find a mistake using the reasoning that made it, and it'll
report that the code is fine. The reviewer has never seen the code before. It
reads the surrounding file to judge the hunk, and answers.

It can only read. Its tools are `explorer`'s, the same scope taken from the
same definition, and its context carries no checkpoint recorder. So a write
isn't something it declines to do. It has no way to perform one. It also
doesn't run anything: no build, no tests.

On a local model a review takes minutes, so **Stop reviewing** sits beside it
while it runs. Closing the drawer ends it too, since nobody's left to read the
answer. A stopped review reports nothing, not half of what it found.

The answer stays in the drawer. It's deliberately kept out of the
conversation, because a review in the transcript is a review in the context
window of every request after it. Avoiding that cost is why the review works
this way. The answer also says what produced it:

```
  2 files read by qwen3.6:27b, without the conversation that produced them —
  so it cannot know what was asked for, and may call a deliberate choice a
  defect.
```

That sentence isn't a disclaimer. It's the trade. A reviewer that could see the
request would be reasoning from the context that wrote the code, and there'd
be no point running it. Any file it wasn't shown (binary, or dropped to fit) is
named under the report. A review that covered four of a turn's six files and
didn't say so would read as a clean bill of health for all six.

The terminal does the same thing: `taurus review` lists what there is, and
`taurus review --turn 3` reads one back. `--model` runs it on a model other
than the conversation's own. That's how you say "check that with the big one"
without moving the conversation onto it.

### Committing a turn

Below the diffs, in a workspace that's a git repository, is the offer to keep
it:

```
Commit message  [ rename Widget to Gadget                      ]

[ Commit this turn ]
Only this turn's files, and only these. Anything you have staged stays staged.
```

The message is seeded from what the turn was asked to do, and you can edit it.
A prompt says what someone wanted, and a commit message says what changed.
Those agree often enough to be a useful start, and rarely enough that
committing one unread is a habit worth not building.

The checkpoint log decides what goes in, not the frontend. The turn is named,
the backend re-reads which files it recorded, and those are the paths
committed. `git commit -- <paths>` is `--only`, so it commits the working-tree
state of exactly those paths and leaves the index alone. If you've staged
unrelated work, it's still staged afterwards. A turn that touched four files
commits four files, however dirty the rest of the tree is.

Three different things stop a path from being committable, and each is
reported in its own words instead of one shrug covering all of them:

```
a1b2c3d  rename Widget to Gadget — 2 files
  not committed  .env — is ignored by git, so it is not in the repository to commit
```

The other two are "already matches the last commit", for a file a later turn
or you put back, and a file that's gone and was never tracked. When *nothing*
survives that filter, the commit is refused, not made empty. The refusal
carries every reason it collected, because "nothing to commit" on its own would
send you looking for a bug that isn't there.

Each commit is offered on its own, and a conversation isn't a single commit.
Committing turn 3 and then turn 5 leaves turn 4's work in the tree,
uncommitted, and now sitting under a commit it isn't in. So the drawer records
which turns are already in `HEAD` and says so before the button:

```
[ out of order ]  Turn 4 changed files and is not committed. Committing this
                  one puts it into history ahead of work it came after.
```

When the turns share a file it says something sharper, because the problem is
worse than ordering. `git commit -- <paths>` commits what those paths hold
*now*. So an earlier uncommitted turn's edits to a shared file go into this
commit wearing this turn's message:

```
[ out of order ]  Turn 4 also changed src/parse.rs and is not committed. This
                  commit takes what those files hold now, so that work goes in
                  with it.
```

A turn that's already been committed is labelled with its commit in the list.
The label survives closing the drawer and reopening the conversation, because
the sha is in the checkpoint log, not in the window. Committing it again is
still allowed, and still says what it would do.

Neither warning refuses anything, and neither offers to squash a run of turns
into one commit. Both stop the silent version.

Committing is refused while a turn is running, for the same reason a rewind
is: the tool calls are still writing.

There's no git *tool*. The model reaches git through `run_command`, where the
permission engine sees the command and the sweep records what it did. A second
path would mean two things to keep in step, and two places to reason about the
sweep's caveat that `.git` isn't restored.

Reading a turn as a diff and committing it belong to the desktop app, not the
CLI. `taurus rewind` still lists and undoes turns, and the diffing and the
commit live in the shared core, so a `taurus commit` is a command away. But a
terminal already has `git diff` and `git commit` a keystroke away, and the
drawer is where you're looking when you decide a turn was worth keeping.

### Conversations know their branch

A new conversation records the branch it was started on, in its transcript
header beside the workspace and the model. Every file path in that
conversation, and every pre-image behind its rewind, describes the tree as it
stood on that branch.

So the rail names a branch only when it's *not* the one checked out now:

```
Fix the parser
on feat/parser · 3 files changed · 2h ago
```

Printing it on every row would make the common case noisier just to make the
rare case visible. That's the wrong trade in a list that dense. Older sessions
that don't record a branch, and sessions started outside a repository, carry no
branch and aren't labelled. Neither is "elsewhere", and guessing would put a
warning on every old conversation.

The rail label isn't the only place the branch shows up. Each turn records the
branch it *began* on into the checkpoint log. It's the branch the turn began
on because a turn that checks out another branch and then edits a file is
exactly the case worth warning about. Recording the branch when the first file
lands would record the destination. A rewind compares those branches against
what's checked out now and says so when they disagree. See
[What a rewind cannot put back](#what-a-rewind-cannot-put-back).
