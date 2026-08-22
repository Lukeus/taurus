# Permission, and undo

<sub>[← Taurus AI Shell](../README.md)</sub>

Nothing here is a confirmation dialog for its own sake. Each of these exists
because an agent that edits files and runs commands needs the answer to two
questions: what is it about to do, and how do I put it back.

## Permissions

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
make](screenshots/permission-diff.png)

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

**Always here is not offered in an untrusted workspace.** That file is not being
read there, so a standing decision would have nowhere to live — see
[Trusting a workspace](#trusting-a-workspace). The call itself is still
allowed; only the standing part of the answer is dropped.

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

## Trusting a workspace

Everything above is about a decision you are asked to make. This one is about
the decisions a folder would make for you.

A workspace's own `.taurus` is not passive data. `mcp.json` starts child
processes. `providers.json` names the endpoint every message of every
conversation is sent to. `search.json` decides whether `fetch_url` may reach
private hosts. A skill can carry a script. And `permissions.json` is a standing
grant — the one file in a repository that hands over capability with no prompt
at all, since `{"allowed": ["run_command:rm"]}` in a clone is an "always allow"
nobody ever clicked.

All of that arrives with `git clone`. So it does not take effect until you say
so:

**An untrusted workspace contributes no config at all.** Not a per-file
carve-out — one rule, in one direction. Your own `~/.taurus` applies in full, so
Taurus works normally in a folder you have not vouched for; what it will not do
is take instructions from it.

The rule being that blunt is what makes it checkable. Every project-tier read in
the harness resolves the workspace through a single function, and a read that
forgot to would be reading `None` — the state that already exists at startup,
before a workspace has been chosen, and that every loader already handles.

**You are only asked when there is something to answer.** A folder with no
config of its own never raises the question, which is most of them. When it does
appear it names what is waiting, and names the MCP command lines rather than
counting them:

```
This project has configuration Taurus is not reading.
  1 skill
  1 MCP server
      probe: npx -y some-package
  2 standing permission grants — tools this project would allow without asking
```

A count of servers is not something anyone can judge. `npx -y some-package` is.

The desktop app puts this in a banner above the composer rather than a modal on
open — nothing from the folder is loaded, so nothing is waiting on the answer,
and a modal to clear before starting work is how a security prompt turns into a
reflex. The terminal prints one line per command and answers with
`taurus trust --allow`. See
[Trusting a workspace](configuration.md#trusting-a-workspace) for the commands.

Declining records nothing. A workspace you waved off and one you have never
opened are the same state on disk, which is what lets a `git pull` that adds a
server ask again.

## Running commands

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

**A machine with no usable pty still runs the command.** Asking for a terminal
and not getting one is not the command's fault, so it runs with ordinary pipes
instead of failing — and the result says so, above the output rather than below
it. That order matters: a program told it is not on a terminal pages nothing and
colours nothing, which reads as a fact about the project unless you already know
the terminal never arrived.

**On Windows the pty is a ConPTY, and a ConPTY is a real console host.** Taken
from the system, that host shows a window when the process asking for it has
none — and a release build of the desktop app has none, so every `pty: true`
command used to open a console window for as long as it ran. The bundle now
ships Microsoft's redistributable ConPTY beside the executable, whose host is
headless, and `portable-pty` prefers it over the system's. It costs 2.2 MB in
the Windows installer: the console host for x64 and a second one for ARM64,
because an x64 build runs on ARM64 Windows under emulation and the host there
has to be native. Neither the CLI nor a development build was ever affected —
both already have a console — which is why this was only ever visible in the
installed app. See [`scripts/conpty.mjs`](../scripts/conpty.mjs).

## Rewinding a turn

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

### Commands are covered too

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

That message reaches the model while the command is running, which is not when
it is needed — the person who needs it is reading a rewind plan, possibly days
later and certainly in another frame of mind. So it is written into the
checkpoint log as well as said, and comes back out at the moment of the undo.
See below.

When a command *cannot* be covered — a workspace past 50,000 files, or one
whose ignore rules the command itself rewrote — the tool result says so in
plain words rather than letting the turn look undoable:

```
[taurus] This workspace holds more than 50000 files, too many to record a
command's changes against, so this one cannot be undone.
```

### What a rewind cannot put back

Restoring files is the whole of what a rewind can do, and three things
routinely make that less than the whole way back. None of them are recoverable
here; all three are knowable, so they are recorded as they happen and reported
beside the plan — before the button, not after it:

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
button, and again afterwards — a commit left pointing at a tree that no longer
exists does not stop being a problem because the rewind finished. `taurus
rewind`'s own listing carries the shorter version, under the turns it is true
of:

```
  turn 2    tidy the caller
            src/main.rs
            committed as a1b2c3d · on feat/parser
  turn 1    teach the parser about tabs
            src/parse.rs
            moved git's own state · on feat/parser
```

Only when there is something to say. A conversation that stayed on one branch
and committed nothing is the common case, and a row of empty fields under every
turn would make the list harder to read rather than more complete.

A dry run produces them identically to a real one, which is the point of having
them. They are ordered by the turns they describe, and a rewind of turns that
only changed files carries none: a warning that fires every time is one nobody
reads.

The branch line compares what each turn recorded against what is checked out
now, so a conversation that never left its branch says nothing about it. A
detached `HEAD` and a workspace outside a repository both count as no branch
rather than as a branch named something — neither gives a name that would still
mean anything quoted back weeks later.

None of this refuses the rewind. It is your tree and there are good reasons to
want the files back regardless; what there is no good reason for is finding out
afterwards.

## What a trace carries

Tracing is off until you name a collector, and naming one sends the *shape* of a
turn: which model, how many tokens in and out, how long each call took, which
tools ran, and which failed. That is a description of the work. It is not the
work.

The conversation is a separate setting, `otlp_capture_content`, and it is off.
Turning it on puts `gen_ai.input.messages` and `gen_ai.output.messages` on every
completion span — which is the file the model read, the command it ran, the
diff it wrote, and whatever you pasted into the composer, sent to whatever
address is configured. There is no redaction pass and there is not going to be
one: a harness cannot know which line of a file it was handed is the secret.

Two things follow, and both are deliberate.

**Nothing infers content capture from an endpoint being set.** They are separate
decisions with different stakes, so they are separate switches. Turning
telemetry on tells you what a turn cost; it never tells anyone what was in it.

**The setting is read per turn, not at launch.** Somebody who has just realized
what they switched on can switch it off and have it take effect on the next
message rather than the next restart.

If you are pointing this at a collector you do not run yourself, leave content
capture alone. The token counts are the part that makes a dashboard useful, and
they are the part that cannot leak a workspace.

## Keeping a turn

A rewind is the way back. This is the way forward, and it is the same list read
in the other direction.

The **Changes** drawer knows precisely which files each turn touched, and it
already holds what they looked like before. That is both halves of a diff, so
every turn can be opened up and read as one:

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
*before* it touched it, so the pre-image the next turn to touch that file
recorded is, by construction, what this turn left behind — and for the last turn
to touch it, what it left behind is still on disk. No post-images, no second
store. The seam is a hand edit between two turns, which lands in the later
turn's diff rather than being attributed to nobody; that is the same assumption
the rewind already states when it warns it will overwrite "anything you changed
by hand since".

A file whose pre-image could not be held — not text, or too large — is named
with its reason rather than left out, the same files a rewind reports as
`skipped`. A turn that looks smaller than it was would be the worst version of
this view.

### Committing a turn

Below the diffs, in a workspace that is a git repository, is the offer to keep
it:

```
Commit message  [ rename Widget to Gadget                      ]

[ Commit this turn ]
Only this turn's files, and only these. Anything you have staged stays staged.
```

The message is seeded from what the turn was asked to do and is editable,
because a prompt says what someone wanted and a commit message says what
changed — those agree often enough to be a useful start and rarely enough that
committing one unread is a habit worth not building.

What goes in is decided by the checkpoint log, not by the frontend: the turn is
named, the backend re-reads which files it recorded, and those are the paths
committed. `git commit -- <paths>` is `--only`, so the working-tree state of
exactly those paths is committed and the index is left alone — someone who has
staged unrelated work still has it staged afterwards, and a turn that touched
four files commits four files however dirty the rest of the tree is.

Three different things stop a path from being committable, and each is reported
in its own words rather than one shrug covering all of them:

```
a1b2c3d  rename Widget to Gadget — 2 files
  not committed  .env — is ignored by git, so it is not in the repository to commit
```

The others are "already matches the last commit", for a file a later turn or you
put back, and a file that is gone and was never tracked. When *nothing* survives
that filter the commit is refused rather than made empty, and the refusal
carries every reason it collected — "nothing to commit" on its own would send
you looking for a bug that is not there.

Each commit is offered on its own, and a conversation is not a single commit.
Committing turn 3 and then turn 5 leaves turn 4's work in the tree — uncommitted,
and now sitting under a commit it is not in. So the drawer records which turns
are already in `HEAD` and says so before the button:

```
[ out of order ]  Turn 4 changed files and is not committed. Committing this
                  one puts it into history ahead of work it came after.
```

When the turns share a file it says something sharper, because the problem is
worse than ordering. `git commit -- <paths>` commits what those paths hold
*now*, so an earlier uncommitted turn's edits to a shared file go into this
commit wearing this turn's message:

```
[ out of order ]  Turn 4 also changed src/parse.rs and is not committed. This
                  commit takes what those files hold now, so that work goes in
                  with it.
```

A turn that has already been committed is labelled with its commit in the list,
which survives closing the drawer and reopening the conversation — the sha is in
the checkpoint log, not in the window. Committing it again is still allowed and
still says what it would do.

Neither warning refuses anything, and neither offers to squash a run of turns
into one commit. Both stop the silent version.

Committing is refused while a turn is running, for the reason a rewind is: the
tool calls are still writing.

There is no git *tool*. The model reaches git the way it always has, through
`run_command`, where the permission engine sees the command and the sweep
records what it did. A second path would be two things to keep in step, and two
places to reason about the sweep's caveat that `.git` is not restored.

Reading a turn as a diff and committing it are the desktop app's, not the CLI's.
`taurus rewind` still lists and undoes turns, and the shared core is where the
diffing and the commit live, so a `taurus commit` is a command away — but a
terminal already has `git diff` and `git commit` a keystroke away, and the drawer
is where someone is looking when they decide a turn was worth keeping.

### Conversations know their branch

A new conversation records the branch it was started on, beside the workspace
and the model in its transcript header. Every file path in that conversation,
and every pre-image behind its rewind, describes the tree as it stood on that
branch.

So the rail names a branch only when it is *not* the one checked out now:

```
Fix the parser
on feat/parser · 3 files changed · 2h ago
```

Printing it on every row would make the common case noisier in order to make the
rare case visible, which is the wrong trade in a list that dense. Sessions
written before this existed, and those started outside a repository, carry no
branch and are not labelled — neither is "elsewhere", and guessing would put a
warning on every old conversation.

The label is where it stops in the rail, but not where it stops. Each turn
records the branch it *began* on into the checkpoint log — began, because a turn
that checks out another branch and then edits a file is exactly the case worth
warning about, and recording it when the first file lands would record the
destination. A rewind compares those against what is checked out now and says so
when they disagree; see [What a rewind cannot put back](#what-a-rewind-cannot-put-back).
