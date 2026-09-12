# Known gaps

<sub>[← Taurus AI Shell](../README.md)</sub>

What Taurus doesn't do, written down so you can read it before you run into
it. Each entry says what's missing and what covering it would cost.

The list grows, but that isn't debt piling up. An entry gets added when a
feature ships and someone writes down where it stops, so a longer list mostly
means more features, honestly described. Most entries are permanent by design
and say so: a terminal has one output stream, `fetch_url` runs no JavaScript,
nothing can make a model write a note. Read those as documentation. The ones
worth watching end by naming something specific that isn't built. Those are
the backlog, and they're the minority.

- **A pty on Windows depends on a runtime fetched at build time.** The Windows
  bundle ships Microsoft's redistributable ConPTY next to the executable:
  `conpty.dll` and a headless `OpenConsole.exe`. The system's own console host
  shows a window when the process asking for it has none, and a release build
  has none. `portable-pty` prefers a sideloaded `conpty.dll`, so those two
  files are the whole fix.

  The cost: 2.2 MB in the installer, a network fetch during a Windows build,
  and a hash-pinned version somebody has to bump. A machine without the files
  still runs every command. The pty falls back to pipes and the result says
  so, but you lose the terminal behavior the call asked for. The app logs at
  startup whether it found the runtime, and that's the only signal. If it's
  packaged wrongly, everything works except for a window that only a Windows
  user on an installed build can see. See
  [Running commands](safety.md#running-commands).
- **A background command's tab is polled, not pushed.** The dock holds one tab
  per command (see [Terminal](capabilities.md#terminal)). While a command's
  tab is on screen, it asks four times a second instead of being told when a
  line arrives. That's a decision, not a stub. The alternative is a
  subscription per job, with a lifetime to get right at both ends, over a
  buffer that's the record anyway. A missed message there would cost nothing
  a later read doesn't repair.

  Polling costs a quarter second of latency on a line, plus a steady trickle
  of IPC calls: one a second with the dock open on the shell tab, and one
  every two seconds with the dock shut while a turn or a command is still
  running. A window with the dock shut and nothing running makes none.
- **What a background command printed is capped at 256 KB.** The buffer is the
  whole record. The tab draws from it and `check_command` reads it, so a build
  that printed more than that has lost its beginning from both. The pane says
  so where the gap is instead of skipping over it. A long test run fits
  comfortably; `cargo build -vv` on a cold cache doesn't. Raising it is one
  number. It isn't higher because the buffer is held per command for as long
  as the command is kept: the eight that may run at once, and the sixteen
  newest that have finished. An older finished command is forgotten, output
  and all, once what it changed has been recorded. Asking for it by number
  says so.
- **A background command's tab can't be typed into.** It's text, not a
  terminal. That follows from the next gap: there's no pseudo-terminal behind
  it, so there's nothing to type into and nothing drawing a screen. You can't
  answer a program that stops to ask a question. All you can do is stop it.
- **A background command has no pseudo-terminal.** `pty: true` and
  `background: true` together are refused, instead of silently doing one of
  them. The pty path runs a command to completion behind a blocking read.
  Handing back a handle instead would mean a second implementation of the
  drain and the stop, per platform. So a dev server that colors its output
  loses the color. A program that refuses to start outside a terminal can't
  be backgrounded at all. You have to run it in the foreground, where the
  timeout applies again.
- **A command still running when a turn ends is in no turn's changed-file
  list.** Its pre-image is held from when it starts and compared when it
  exits, so nothing is lost: the changes land in whichever turn is running at
  the next tool call after it finishes. But a rewind offered mid-build can't
  include what the build hasn't written yet, and the list you read before
  deciding says nothing about the command about to add to it. Covering it
  means a changed-file list that grows as you read it: a UI question, not a
  recording one.
- **A file a background command and another call both changed belongs to the
  other call.** The command's comparison runs from its start to its exit, so
  it sees every edit made in that time. Edits other calls record stay with
  their own turns instead of being recorded again with the command's older
  pre-image. The cost is the command's own change to such a file. Undoing the
  command's turn doesn't touch it. Undoing the other call's turn restores the
  file as it stood then, the command's change so far included.
- **What a message costs is estimated, at four characters a token.** The fixed
  part of a request is measured. A response reports the whole prompt's size,
  and its difference from the estimate for the same messages is exactly the
  system prompt, the tools, and the envelope. What isn't measured is the drift
  *inside* the messages. A tokenizer that gets 3.2 characters a token out of
  minified JSON leaves the estimate a fifth low on a conversation full of it.
  The overhead can't absorb that, because it grows with the messages instead
  of sitting beside them.

  Closing it takes one of two things. Either a tokenizer per model in the
  harness, kept in step with every backend forever, or a count-tokens round
  trip before each request, which is the cost the estimate exists to avoid.
  The threshold covers the error, and the meter above the composer shows you
  when it's wrong.
- **A hook can refuse a tool call but can't approve one.** There's no `allow`
  verdict, so a hook can't skip a permission prompt the way hooks in some
  other harnesses can. That rules out "approve every `git status` for me", and
  the trade is deliberate. A hook that could approve would make `hooks.json` a
  second permission surface, trusted exactly as much as `permissions.json` and
  kept in step with it. As it is, adding hooks to a machine can only shrink
  what it will do, and that's what lets Taurus honor a project's hook file
  at all. This covers the narrowing use: "never let it force-push". See
  [Hooks](configuration.md#hooks).
- **A hook that can't run blocks the call.** A missing program, a crash, or a
  timeout denies the call on the two events where there's still something to
  deny. So a typo in `hooks.json` stops every call it matches until you fix
  it. That's intended, not an oversight: a guard that treats its own breakage
  as approval stops guarding at the one moment it matters. But it does mean a
  hook can break a working setup, which a purely observational one couldn't.
  `taurus hooks check` names the entry and the field.
- **Hooks aren't told about a delegate's turn boundaries.** A sub-agent's tool
  calls go through the same `pre_tool_use` and `post_tool_use` hooks as the
  parent's. The context is shared, which stops a delegate routing around a
  guard. `user_prompt_submit` and `stop` fire for the conversation, not once
  per child. From the outside a delegation is one tool call, and firing "the
  turn ended" four times for one turn would break any `stop` hook that
  counts. Covering it properly needs an event pair of its own, and nothing
  needs one yet.
- **Trust is per folder, and it's answered once.** A workspace you've vouched
  for stays vouched for. So a `git pull` that adds a server to
  `.taurus/mcp.json` is read on the next turn without asking again.

  The fix would be fingerprinting the config and asking again whenever it
  changes. That sounds strictly better and isn't. The file changes on
  ordinary branch switches, and a prompt that shows up on most
  `git checkout`s gets clicked through, including the one time it matters. So
  the decision on offer is "this project may configure Taurus", the same unit
  an editor's workspace trust uses. Read honestly, it's trust in the
  project's maintainers, not in a particular revision. `taurus trust --revoke`
  and the Settings row withdraw it. See
  [Trusting a workspace](safety.md#trusting-a-workspace).
- **Trusting a workspace is about its config, not about its code.** The gate
  decides whether a folder's `.taurus` may configure the harness. It says
  nothing about what running the project's build script does, and it can't.
  Once you ask an agent to work in a repository, the repository's own code is
  what you asked it to run. Commands still go through the permission prompt,
  which is where that decision is really made. An untrusted workspace isn't a
  sandbox, and nothing here calls it one.

  The banner reads those config files and names anything in them worth
  looking at. That narrows the gap but doesn't close it: a finding is
  something to read, and *no* findings isn't a clean bill of health. The rules
  are deliberately quiet. A zero-width joiner is how a family emoji is built,
  and a `data:` image is a long run of base64, so neither fires. Quiet rules
  miss things by construction, and a technique nobody wrote a rule for
  arrives unflagged. See
  [Trusting a workspace](safety.md#trusting-a-workspace).
- **The config scan reads the first 64 KB of at most 64 files.** It runs
  inside the same `pending` call the desktop app makes on every refresh and
  the CLI makes once per command. So it's capped three ways: files opened,
  bytes per file, and findings reported. A payload past any of those isn't
  seen. Raising the caps is three numbers. They're where they are because the
  alternative is reading an unbounded tree on every `taurus run`, on a path
  whose whole job is to print one line and get out of the way. The scan never
  runs at all on a workspace with no config of its own, which is most of
  them.
- **Nothing re-reads trust between turns on its own.** The desktop app asks
  the backend for it when it refreshes, and the CLI asks once per command. A
  workspace that gains its first `.taurus/mcp.json` while the window is open
  raises its banner at the next refresh, not the moment the file lands.
  That's the same turn-boundary rule the rest of the config follows, and
  there's no watcher here for the same reason. See
  [Trusting a workspace](configuration.md#trusting-a-workspace).
- **A review reads the code and not the intent.** **Review this turn** hands a
  diff to an agent that has none of the conversation. That's the only way the
  exercise means anything, and it's also the whole limitation. The reviewer
  can't know what was asked for, so it will call a deliberate choice a
  defect: a function left unused on purpose, an error swallowed because the
  caller handles it, a simplification you asked for by name. Its brief tells
  it to say "if this was intended, ignore me" instead of asserting. That
  softens the wording, not the fact. Covering it means giving the reviewer
  the request back, which gives it the context that wrote the code, and then
  there'd be no point running it. See
  [Reading it back](safety.md#reading-it-back-to-somebody-who-did-not-write-it).
- **A review reads; it does not run.** Its tool list is `explorer`'s: no
  build, no tests, no reproducing anything. It's told not to claim a test
  passes, but that's a prompt, not a guarantee. What stops it *changing*
  anything is structural, not an instruction. The scope holds no writing
  tool, and the context carries no checkpoint recorder, so there's nothing to
  record a write into.
- **A review sees at most 24 KB of diff, and at most 160 lines of any one
  file.** The per-file cap is `FileDiff`'s own. The total is the reviewer's,
  and it keeps whole files instead of cutting one mid-hunk, because a diff
  that stops in the middle reads as a change that ends there. Everything left
  out is named in the report, not dropped. A single file over the cap is
  still reviewed whole, because refusing the largest turn means refusing the
  one most worth reviewing. A turn past both caps gets a partial review that
  says which parts it covers.
- **A review costs a model round trip, on your own provider.** It's a button
  that takes a minute on a local model. Nothing about it is free or cached:
  asking twice asks twice. It's deliberately not a roster line the model can
  reach. A `reviewer` sub-agent would add a line to every request's
  spawn-tool description, charging every conversation for a button pressed
  once an hour. The trade is that the model can't decide to review something
  on its own. Only you can.
- **A rewind does not cover ignored directories.** A file an ignore rule
  excludes by name is covered. Anything under a directory an ignore rule
  excludes isn't, so a command that rewrites something in `target/` or
  `node_modules/` is neither listed nor restorable. Widening it would mean
  indexing those before every command, which isn't affordable, and a rewind
  deleting build output, which nobody wants. See
  [Rewinding a turn](safety.md#rewinding-a-turn).
- **A rewind reports git state, it does not put it back.** `.git` is left out
  of the walk. So undoing a turn that ran `git checkout` or
  `git reset --hard` restores the file contents but leaves `HEAD` and the
  index where the command moved them. You get a tree that matches neither
  commit. Covering it properly means snapshotting the object store, which is
  a feature of its own.

  What you do get is the warning at the right time. The sweep writes the fact
  into the checkpoint log, and the rewind plan repeats it beside the file
  list. So you hear about it when you reach for undo, not only when the
  command ran. Staging isn't reported. See
  [Rewinding a turn](safety.md#rewinding-a-turn) for why the index is
  deliberately not watched.
- **A change that moves neither length nor timestamp is invisible.** The same
  walk compares size and modification time, which is what `make` and `rsync`
  have always compared. On a filesystem with nanosecond timestamps, defeating
  it takes deliberate effort. On one with coarse timestamps, a command that
  rewrites a file to the same length within the same tick would slip through.
  Closing it means reading every file twice per command.

  Every command after the first in a workspace reuses what the previous one
  read, keyed on the same length and modification time. The host keeps that
  cache across turns, so a workspace is read once, not before every command.
  Same comparison, same blind spot, but it reaches one case further. A sweep
  alone would only fail to *notice* an invisible change; a reused read can
  also carry the wrong pre-image. If a file is rewritten to the same length
  and timestamp between two commands, and a later command in the same turn
  changes it visibly, a rewind puts back the version from before the
  invisible edit.

  It's bounded at both ends. It takes a deliberate same-length, same-tick
  rewrite in the window between two commands. And a file the turn already
  recorded is unaffected, because a turn keeps its first pre-image. The fix
  is the same: read every file twice, in the same place.
- **A pty command's stdout and stderr can't be told apart.** A terminal has
  one stream, so `pty: true` gives up the `[stderr]` split the piped path
  reports. That's the format, not the implementation, and it's why the pty is
  opt-in and not the default. On both paths, output streams to the transcript
  as it's produced. It's batched every 100ms and kept as a bounded
  scrollback. If the UI falls behind, lines are dropped from the *display*
  instead of stalling the child. The model always gets the complete output.
  Only what you're watching scroll past can skip.
- **A pty command answers prompts it was given, not prompts it wasn't.**
  `stdin` is written up front and closed, so a program that asks something
  unexpected still waits for the timeout. A real back-and-forth would mean
  keeping the turn open on a running child and deciding what the model may
  type into it. That's a bigger surface than this opens.
- **Reasoning the provider returns redacted can't be replayed.** Anthropic
  signs its thinking blocks and requires them back unedited. A redacted one
  has no signature this harness can carry, so it's left out of the next
  request. Where that matters, the API says so explicitly instead of failing
  quietly, but the turn has to be retried. Carrying the encrypted form would
  mean a second shape in the normalized types for one provider's edge case.
- **An instructions file is read at turn boundaries, not watched.**
  `AGENTS.md` and everything it imports are re-read at the start of each
  turn, so an edit lands on your next message, not the next reload. It isn't
  *watched*, and that won't change. A watcher fires whenever your editor
  saves, which is often mid-turn, and a turn has to finish on the brief it
  started with.

  That leaves a one-turn delay and one narrow blind spot. The freshness check
  compares length and modification time, so a rewrite to the same length
  within one filesystem tick waits for the next change. The sweep makes the
  same comparison, and closing it takes the same fix: reading every file on
  every message, which is what the check exists to avoid. See
  [Instructions](capabilities.md#instructions).
- **Warnings are matched by their headline, which is not the same as by their
  lint.** Two hundred sites of one clippy lint print the same headline and
  collapse to one body. Two hundred `unused variable` warnings each name a
  different variable, so each is its own headline and none collapse. That's
  the weaker half of the feature. It's a limit of the text, not the code:
  rustc prints the `#[warn(...)]` note that names the lint only on the lint's
  first occurrence.

  `--message-format=json` would close it, but the model would have to ask
  for it. Rewriting a command line to add a flag isn't the same as shortening
  its output, and it fails differently: the command that ran wouldn't be the
  command you approved. See
  [Output too big to hand over](safety.md#output-too-big-to-hand-over).
- **The test filter knows libtest and nothing else.** It recognizes the
  format `cargo test` prints, which covers a Rust workspace and a test binary
  run directly. `cargo nextest`, `jest`, `pytest` and `go test` all announce
  themselves differently. Their output is collapsed only where it repeats
  itself, which for a passing suite is never. Each would be a small filter
  beside the existing one. They aren't written because none of them can be
  checked against real output from this machine, and a pattern nobody has run
  against what it matches is a guess. See
  [Output too big to hand over](safety.md#output-too-big-to-hand-over).
- **Only repetition that is literally consecutive is collapsed.** The pass
  runs on output large enough to be worth it: 16 KB on a 200,000-token model,
  and a share of the window on any other. A line printed three or more times
  in a row becomes one copy and a count. That covers most of a chatty build,
  and a server printing one failure line over and over. It doesn't cover most
  real server logs. Two alternating messages, like a retry and the
  timestamped line after it, are never adjacent, so nothing collapses.

  Closing that means matching lines that are merely similar, a different kind
  of claim. A count of identical lines is a fact; a count of lines that
  looked alike is a judgment the model can't check.

  None of this applies to `check_command` either. A background command's
  readers keep byte cursors into a shared buffer, and shortening the text
  those cursors count would move the model's place and the window's apart.
  See [Output too big to hand over](safety.md#output-too-big-to-hand-over).
- **A cut command's output can be swept away while the transcript still points
  at it.** When a stream outruns what the model's window has room for, the
  whole of it is written to `~/.taurus/output/<workspace-key>/`, and the gap
  in the result names the file. So the middle of a long build is a
  `read_file` away, not a re-run. But twenty streams are kept per workspace,
  oldest out first. A message from earlier in a long session can name a file
  a later command has displaced, and a conversation reopened weeks later
  usually finds nothing there. The model is told the file is missing instead
  of being shown the wrong one, which is the important half.

  Closing the rest means either keeping build logs indefinitely or pruning
  against the transcripts that reference them. That's a second index, of the
  kind [Sessions](../crates/taurus-host/src/sessions.rs) deliberately doesn't
  keep. Size isn't the other half: `read_file` windows around the offset it's
  given, so a spilled stream opens at any line however large the file. See
  [Output too big to hand over](safety.md#output-too-big-to-hand-over).
- **A diff is shown for `write_file` and `edit_file` and nothing else.** A
  command line has no before-and-after to compute, which is exactly why
  `run_command` is swept afterwards instead of predicted. So the most
  consequential writes in a session, the ones a script made, are approved on
  the command line alone. You can at least *read* them afterwards. The
  **Changes** drawer diffs what the sweep recorded, so you can inspect a
  `sed -i` across a dozen files line by line once it's happened. That's
  review after the fact, not before it.
- **A turn's diff attributes a hand edit to the wrong turn.** What a turn
  changed is computed as its own pre-image against the next recorded
  pre-image of the same file. That's exact for anything Taurus did and
  silently wrong for anything you did in between: your edit shows up in the
  later turn's diff. Closing it means post-images, a second copy of every
  file written per turn, to attribute a case the rewind already warns about
  in the same words. See [Keeping a turn](safety.md#keeping-a-turn).
- **Committing a turn does not offer to squash.** The checkpoint log records
  which turns are in `HEAD`. So committing turn 5 while turn 4 is uncommitted
  warns you first. The warning is sharper when the two share a file, because
  `git commit -- <paths>` takes what those paths hold now and would carry
  turn 4's edits in under turn 5's message. What's missing is the other half:
  nothing offers to commit a run of turns as one. That isn't a record shape.
  It's a second commit path with its own message editor and its own failure
  modes, and it isn't built. See
  [Committing a turn](safety.md#committing-a-turn).
- **A committed turn can still be rewound, deliberately.** Rewinding past a
  turn you committed restores the files and leaves the commit in place, so
  the tree doesn't match it. That won't change: it's your tree, and there are
  good reasons to want the files back anyway. The rewind plan names the
  commit and what to do about it (`git revert`, `git reset`) before you press
  anything, so you don't find out afterwards. The one thing it won't do is
  refuse. A rewind that second-guessed you would be a worse tool than one
  that tells you.
- **Nothing makes a model write a note.** `remember` is offered and the prompt
  says when to use it, and that's as far as the harness's leverage goes: the
  same limit `update_plan` has, for the same reason. A model that finishes a
  long piece of work and just answers leaves nothing behind, and the next
  conversation starts blank. Forcing a note every turn would spend an
  iteration on the many turns with nothing worth carrying, and fill the
  drawer with notes nobody needed.
- **A note isn't checked against the workspace it describes.** It says what
  was true when it was written, and nothing revisits it. A note about a
  branch that has since merged, or a file since deleted, goes into every
  later conversation just as confidently as one that's still true. The prompt
  says so plainly, which puts the judgment on the model, where it has to be.
  The Memory drawer is where you remove a stale one. Expiring notes
  automatically would need a model of what each note is *about*, which is a
  bigger claim than a line of prose supports.
- **Nothing makes a model plan.** `update_plan` is offered and the prompt says
  when to use it, and that's as far as the harness's leverage goes. On a
  five-step mechanical task, neither `qwen3.6:27b` nor `qwen3.5:9b` called it
  unprompted. Both just did the work. So the feature earns its keep on long,
  exploratory turns where drift really happens, and on models that follow the
  instruction. Forcing a plan on every multi-step request would spend an
  iteration and a card on turns that never needed one.
- **A plan can still end in progress — but not silently.** A model that
  finishes the work and goes straight to its answer can leave a step reading
  `[>]` that's actually complete. The pinned panel keeps that stale version
  where you can't miss it. The harness can't decide a plan is finished, so it
  does the one thing it can. When the model tries to end the turn with steps
  open, it's asked once to send the list back closed, and the turn continues.
  That's the same lever as the verify nudge, with the same limit. A model
  that answers without calling `update_plan` gets to stop, and the panel
  clears on the next request.
- **A plan doesn't survive the process.** The board is held per session in
  memory, not written to disk. An unfinished plan carries across messages and
  is gone if the app restarts. Reopening the conversation redraws the panel
  from the transcript, because the panel is derived, but the model has to
  rebuild its own copy. Persisting it would mean a second record that can
  disagree with the tool calls that made it, which is a worse failure than
  re-deriving.
- **Whether a carried plan still applies is the model's call.** The harness
  clears a finished plan and labels an unfinished one as belonging to an
  earlier message. It can't tell whether a follow-up continues the task or
  changes the subject. So a model that ignores the label works the old
  checklist against the new request.
- **A branch is warned about, not enforced.** A conversation started on
  `feat/x` and resumed on `main` is labeled in the rail. Each of its turns
  records the branch it began on, and a rewind that would write those
  pre-images over a different tree says so beside the plan. It doesn't
  refuse, and its file references still point where they pointed. Refusing
  isn't built and probably shouldn't be: a rewind onto another branch is
  occasionally exactly what someone means, and the warning is what separates
  that from an accident.
- **A sub-agent's answer is summarized, not streamed.** Its tool calls appear
  under the delegation card as it makes them, so a long delegation looks
  alive, not hung. Its reasoning and prose stay inside the child. That part is
  deliberate: the parent asked for a conclusion, and inlining a second
  conversation into the transcript is what delegation exists to avoid.

  You can still read it afterwards. Every delegate keeps its own transcript
  beside its parent's, written as it runs. The delegation card opens it in a
  drawer while the call is running or long after it's finished.
  `taurus sessions --agents <ID>` lists them in the CLI. The CLI prints where
  they are instead of rendering one, because the terminal has no second pane
  for a conversation inside a conversation.

  A *resumed* conversation loses the link, not the transcripts. The parent's
  own record says a delegation happened, not where its child was written, so
  a reopened conversation's cards don't offer to open one. The files are
  still there, and `--agents` still lists them.
- **A custom agent's roster is frozen for the turn, on purpose.** The set of
  sub-agents is snapshotted when a turn starts, so an agent file saved
  mid-turn isn't visible until the next one. That's all. The directories are
  checked at every turn boundary and rescanned when anything in them changed,
  so a new agent is available on your next message, without a reload or a
  trip to the drawer. The freeze is the feature: a turn has to delegate
  against the roster it started with. That's why there's no file watcher
  here, and it isn't a missing piece. The same-length-same-tick blind spot
  above applies to this check too.

  One surface lags on purpose: the `/` command *menu* lists the last scan. It
  redraws on every keystroke, and taking config locks there is how a reload
  deadlocks against typing. Typing the name in full works immediately. The
  menu re-reads itself whenever a rescan changes how many skills or agents
  there are: when a message finishes, when a drawer opens, or when you come
  back to the window. See [Sub-agents](capabilities.md#sub-agents).
- **A proposed agent's system prompt is reviewed by eye, and nothing else.**
  `propose_agent` checks the shape: the name, the description, the tool
  scope, and whether it duplicates an existing agent. But the prompt itself is
  prose, and it will steer a delegate on every future turn. `propose_skill`
  has the same exposure and the same answer. The card shows the prompt in
  full, unelided and editable, and nothing is written until you approve it.
  Nothing checks what it says. See [Sub-agents](capabilities.md#sub-agents).
- **Reading another client's directories is not the same as being that
  client.** Taurus reads Claude's and GitHub Copilot's skills, sub-agents, and
  standing instructions, because it already understands all three formats. It
  doesn't behave like those clients.

  Copilot's scoped `*.instructions.md` files declare an `applyTo` glob and
  are attached when Copilot is about to touch a matching file. Taurus
  assembles a brief once per turn, before it knows what the turn will touch.
  So it carries the glob into the prompt as a sentence and leaves the model
  to apply it. That's a weaker guarantee than Copilot's, and the file says so
  plainly instead of acting as if it were the same.

  Frontmatter keys these tools have and Taurus doesn't (`handoffs`, `hooks`,
  `user-invocable`) are ignored, not honored. That's why a borrowed file is
  never rewritten in place. `.claude/rules`, Claude's spelling of the same
  scoped instructions, is the one directory in this family Taurus doesn't
  read. See [Instructions](capabilities.md#instructions).

- **The agent won't install an MCP server for you.** `draft_mcp_server` hands
  back an entry, and adding it is up to you, in the MCP panel or in the file.
  The command line is all a review could show, and it doesn't say what the
  program does. So this is a limit, not a to-do. The catalog behind
  **Browse servers** answers the same argument from the other end. The review
  happens once, in a commit, against the source. A person can do that
  properly; a model emitting a package name at runtime can't. See
  [MCP servers](configuration.md#mcp-servers).
- **A disabled server's cost is unknown, not zero.** The MCP panel prices each
  connected server's tool schemas: what it adds to every request, called or
  not. It shows nothing for a server that never connected. That's the honest
  answer, not a rendering gap: a disabled server registers no tools, so
  there's nothing on the live registry to measure. The panel answers "what
  did enabling this cost me", but before flipping a switch you want "what
  would enabling it cost". Answering that means starting the program and
  asking it, which is what the switch is off to avoid. The figures are also
  estimates at four characters a token, like every other token count outside
  the **Billed** row. See
  [The MCP panel](configuration.md#the-mcp-panel).
- **Taurus speaks the tools half of MCP and none of the rest.** A server's
  tools are found, namespaced and offered to the model. Its *resources*,
  *prompts* and *roots* aren't asked for. Nothing listens for
  `notifications/tools/list_changed`, so a server that gains a tool
  mid-session isn't noticed until the next Reconnect. And the two requests a
  server can make of the client, *sampling* and *elicitation*, go
  unanswered. Tools are what almost every server leads with, which is why
  they're here. It isn't why the rest is missing. Prompts and roots are the
  two worth having next. Prompts, because a server's own prompts belong in a
  shell's command line. Roots, because a filesystem server told once where it
  may look beats one told again on every call.
- **A dead server's tools stay callable until you reconnect.** The panel
  updates the moment a call finds the server gone. But the tools were handed
  to the tool registry, and nothing revokes them there. So the model can
  still call one, and gets back a sentence saying the server isn't running
  and Reconnect is the fix. Taurus doesn't restart it automatically. A server
  that dies on startup would restart in a loop, and nobody has written a
  policy for when a crash is worth retrying.
- **A server that hangs isn't marked dead, only the call is.** The line is
  drawn at the transport. A closed pipe or a dropped connection means the
  server is gone, and the panel says so. A server that ignores a request for
  two minutes may well answer the next one, so its card is left alone and
  only the call fails. That's a judgment, and it can be wrong in one
  direction: the panel can look healthier than the server is.
- **A token you paste is stored in `mcp.json` in plain text.** OAuth refresh
  tokens go to the OS keychain. A personal access token typed into the header
  or environment field doesn't. It's written into the file, the same file the
  panel exists to save you from editing. Entries that want a token default to
  the global file, not the one inside the repository. Choosing the repository
  asks you to acknowledge that a commit can publish it. There's no keychain
  reference. `${VARIABLE}` is expanded in any field, so the way to keep a
  token out of the file today is to export it and name the variable. Nothing
  in the panel tells you that.
- **A sign-in needs the authorization server to register Taurus on the spot.**
  OAuth works through dynamic client registration (RFC 7591): Taurus asks the
  authorization server for a client id when you press Sign in. The current
  specification prefers a Client ID Metadata Document instead, which is an
  HTTPS URL the authorization server fetches to learn about the client. A
  desktop application has nowhere to host one, so that route is closed until
  Taurus has a published document to point at. A server that offers neither,
  and expects a client id issued by hand through a developer console, can't
  be signed in to. The panel says so instead of failing at the redirect. If
  that server also issues a plain token, the HTTP form takes it.
- **Signing out is local.** It removes Taurus's copy of the tokens from the
  keychain. The grant itself lives at the authorization server. It stays
  there until you remove the application in that provider's own settings,
  the only place it can be revoked. The panel says this instead of implying a
  revocation it can't perform.
- **Scope step-up isn't wired to anything.** When a server answers a tool call
  with `insufficient_scope`, that error surfaces as the call failing. The
  library underneath supports re-authorizing for a wider scope. What's
  missing is the decision it needs: asking somebody mid-turn to widen a grant
  is a permission prompt of its own, and it isn't built.
- **The catalog is a snapshot, and it's small on purpose.** Around a dozen
  entries, shipped in the binary, all first-party: the MCP project's own
  reference servers, plus GitHub's and Brave's. Nothing third-party is
  listed, because a third-party package name is exactly what
  `draft_mcp_server` refuses to ask anybody to approve. It goes out of date
  between releases, and the panel says when it was last checked. It can't
  break a working setup: an install copies the entry into `mcp.json`, and the
  catalog never reads it again. A registry search would fix the staleness
  and bring back the unreviewed-package problem. If one is added, it'll be
  marked as such and land in the manual form, not the guided one.
- **A PATH read from your login shell is a snapshot, not a subscription.**
  Taurus asks the shell once at startup, because a window launched from the
  Dock inherits the launcher's PATH, not yours. A server installed after that
  (`npm i -g` in a terminal beside the app) is invisible until you restart
  Taurus, or until the entry names the program by its full path. The MCP
  panel shows which directories it's searching, so you don't have to guess.
- **An agent's tools narrow what it's offered, not what it may do.** Every
  call a child makes goes through the same permission engine as the parent's,
  so `tools:` is a scope, not a sandbox. A per-agent permission policy would
  be a second thing to keep in step with the first, and there isn't one.
- **Stall detection needs an exact repeat.** Alternating between two dead ends
  is caught, but the calls have to match argument for argument. Say a model
  asks the same unanswerable question three slightly different ways, like
  reading a missing file by three spellings of its path. It's making no more
  progress than one asking identically, and nothing here notices. Catching it
  would mean deciding when two calls are *near* enough to be the same
  mistake. That's a guess, and the iteration ceiling makes it unnecessary. See
  [When a turn stops](working-with-it.md#when-a-turn-stops).
- **The *model* can't produce an image.** A tool can hand one back, but the
  model only reads pictures. It can't draw or edit one. So a turn best
  answered with a diagram answers in prose or uses `show_chart`. Closing that
  means image *generation*, which only some backends offer, and none the
  same way.
- **No built-in tool returns an image yet.** The shape exists and every
  adapter carries it, but only MCP servers use it: a browser driver's
  screenshot, a renderer's output. A built-in that rasterizes a PDF page or
  captures a window is a tool nobody has written here, not a limit on what a
  tool may return.
- **Only Anthropic carries a tool's image inside the result.** OpenAI's
  `role: "tool"`, Gemini's `functionResponse`, and Ollama's tool message are
  text. On those three, the picture moves to just after the result, with a
  marker line where it was and a note naming the call. It arrives in order
  and attributed, but as a separate part of the conversation, not part of the
  answer. A model that weighs a tool result differently from a user message
  will weigh it differently. That's what the wire formats allow. Closing it
  needs those APIs to change, not this one.
- **A tool's image is budgeted at a flat estimate, like a pasted one.** 1,000
  tokens, whatever its dimensions. The real cost has nothing to do with the
  length of its base64, and each provider prices it differently. So the
  number the compaction trigger reads is approximate exactly where the stakes
  are highest. A turn that returned four screenshots may have less room left
  than the counter says.
- **Trimming an old tool result drops its picture.** That's deliberate. The
  point of shortening an old result is to reclaim the window, and the image
  is the most expensive thing in it. But a screenshot from earlier in a long
  conversation disappears from the model's view while its caption stays, and
  nothing says which it was.
- **An attached image isn't in the checkpoint log.** It goes into the
  transcript, so it survives and redraws. It isn't a file in the workspace,
  so a rewind neither restores nor reports it. That's correct, since there's
  nothing to put back. But a conversation's disk footprint grows somewhere
  the **Changes** drawer doesn't account for.
- **The first index is slow, and a search that arrives early still waits for
  it.** Embedding this repository takes nearly two minutes. Sending a message
  starts it in the background, so most of it is usually done before anything
  searches. But a model that calls `search_code` in its first tool call waits
  for the rest inside that call. The wait is shorter than a fresh start. The
  search takes over the warm-up instead of starting again, everything
  embedded so far is already written down, and the turn shows a passage count
  moving. Closing it fully means answering from a partial index and saying
  so. That's a different promise from the one the tool makes: every search
  refreshes first, so a file you just wrote is a file it can find.
- **The index doesn't notice a file that changed without moving.** It compares
  length and modification time, like the sweep, and it's blind in the same
  place. A rewrite to the same length within one filesystem tick is
  invisible, and the stale chunk stays until something else about the file
  changes.
- **Semantic search is only as good as what ranks it.** `search_code` ranks by
  cosine similarity, and optionally by a reranking model over the top thirty
  of those. There's no keyword fallback and no blend with grep. A query that
  lands badly returns three confident near-misses. The tool says they're
  leads, not answers, and that's all it can do about it. Where you know the
  literal text, grep is exact and this is only close.
- **Nothing here embeds in-process.** An index needs a backend with an
  embedding endpoint, and every provider Taurus speaks to except Anthropic
  has one. So this is a gap for exactly one setup: chatting to Claude with no
  other backend reachable. Closing it means running an embedding model inside
  this process. That's a model to download on first use and a
  machine-learning runtime to carry on all three platforms, for a case a
  second provider entry already covers. It isn't built, and it's not
  obviously worth building.
- **Reranking needs a second server, and most backends can't be it.** The
  `/rerank` route is Cohere's shape, not OpenAI's, and OpenAI has none to
  imitate. So it's served by llama.cpp started with `--reranking`, by
  text-embeddings-inference, by the hosted rerankers, and by almost nothing
  else. Ollama, where most local setups embed, has no such route at all.
  That's why `rerank_provider` is a setting separate from the embedding
  provider. Closing this properly means running a reranking model in-process
  instead of asking for an endpoint. That's the same unbuilt piece that would
  let the index work with no local server at all.
- **A reranked score can't be compared to anything but its own result set.**
  Voyage and Cohere normalize to 0–1. llama.cpp returns the cross-encoder's
  raw logit, where negative values are ordinary. Taurus orders by it and never
  filters on it. It labels the column `relevance`, not `similarity`, so
  nobody reads the number as a cosine. But there's no way to make one
  backend's 0.82 mean the same as another's, and no threshold below which a
  result is known to be worthless.
- **The Traces panel covers one run of the app, and nothing else.** The spans
  it draws live in a ring in that process. Quitting forgets them. A
  `taurus run` in the terminal is a different process and never shows up in
  the window. And once the ring is full, the oldest go. The panel says how
  many it's forgotten, so it never looks like it covers a longer period than
  it does. But "everything since launch" is the widest question it can
  answer. Anything longer than a session is what an OTLP endpoint is for. The
  two aren't alternatives: the same spans go to both.
- **Traces go out over HTTP, and only over HTTP.** OTLP has a gRPC transport
  too, and Taurus speaks only the `http/protobuf` one. Every collector worth
  naming accepts it, so the gap is smaller than it sounds. But a setup
  standardized on gRPC needs a collector in front. And the port is the other
  one (4318, not 4317), which is the mistake everybody makes once.
- **A trace says which tools ran, not what they were called with.** Tool spans
  leave out arguments even when content capture is on. Arguments are the one
  place a path, a URL, or a command line would end up on a dashboard with no
  way to notice. Closing it means deciding what an argument may contain,
  which is the same unanswerable question redaction always is.
- **Cache and reasoning tokens are only as good as the backend's report.**
  Anthropic reports cache reads and writes. OpenAI-compatible servers report
  cached prompt tokens and reasoning tokens when they have them. Gemini
  reports cached content and thoughts. Ollama reports none of it, and a
  compatible gateway may report a subset or nothing. Absent is recorded as
  absent, not zero. But that means a dashboard comparing two backends is
  comparing what each chose to say.
- **Reasoning tokens are inside the output count, not beside it.** Every
  backend that reports both counts reasoning within `output_tokens`, so
  adding the two double-counts. The field stays because it's the only way to
  see that a turn spent its budget thinking instead of answering.
- **`fetch_url` reads the HTML it's served.** No JavaScript runs, so a page
  that renders its content client-side comes back near-empty. Closing this
  means shipping a browser engine, so it's a limit, not a to-do.
- **`fetch_url`'s address check doesn't survive a proxy.** Loopback and
  private-network addresses are refused. The check runs inside the client
  that connects, so a name can't answer publicly for the check and privately
  for the connection. An HTTP proxy resolves the name at its end, though, so
  a request routed through one reaches a destination Taurus never sees.
  Taurus configures no proxy, but reqwest reads `HTTP_PROXY` and the system
  settings, and refusing to work behind a corporate proxy would cost more
  than this buys. `"allow_private_hosts": true` in `search.json` turns the
  check off deliberately.
- **Config is re-read at turn boundaries, and nothing is watched.**
  Instructions, sub-agents, skills, and hooks are all fingerprinted (a `stat`
  of the files behind each) and re-read at the start of a turn when the
  fingerprint changed. So an edit lands on your next message, not the next
  launch. Coming back to the window runs the same check too.

  There's deliberately no file watcher. A watcher fires whenever your editor
  saves, which is often mid-turn, and a turn has to finish on the brief and
  the roster it started with. The costs are a one-turn delay at worst, and
  the same same-length-same-tick blind spot every fingerprint here has.

  Two things *aren't* on this path and need a reload: the provider list and
  `settings.json`. You edit both in the app most of the time, not in a file,
  and both are re-read when you save them there. A hand edit to
  `providers.json` while the app is open is the case that waits.

- **The terminal dock is a terminal, not part of the conversation.** It runs
  your shell in the window the agent works in, and that's the whole
  connection. The agent can't read what you ran there. You can't hand it a
  failed command without copying the text across. And its own `run_command`
  calls show up in the transcript, not in the pane. All three come from the
  same missing piece: the shell has no way to say where one command ended and
  the next began, so there's nothing for either side to point at.

  Closing it means shell integration: the `OSC 133` marks a prompt emits
  around each command, injected per shell. That would turn scrollback into
  addressable blocks, each with an exit code and a duration. It's the next
  thing to build here, not a limit. See [Terminal](capabilities.md#terminal).
- **What you run in the terminal is outside the undo history.** Every command
  the *agent* runs is bracketed by a sweep of the workspace, so a rewind can
  put back anything it changed. A command you type in the dock isn't. The
  shell runs it directly and nothing reads the workspace before or after, so
  a `sed -i` there is invisible to the Changes panel and to every checkpoint.
  The dock doesn't pretend otherwise. It's a terminal, and a terminal has
  never had an undo. But the two halves of the window keep different
  promises. Covering it needs the same command boundaries as the entry above,
  and it would cost a read of the workspace per command you type. See
  [Rewinding a turn](safety.md#rewinding-a-turn).
- **One shell, and it ends when the dock does.** There are no tabs and no
  splits. Hiding the pane isn't hiding it: closing the dock ends the shell,
  the same as closing a terminal window. So a long `cargo build` started
  there doesn't survive ⌃`, and there's no second pane to run something else
  in while it works. Both are worth having and neither is written. A shell
  that outlived the pane would also need somewhere for its output to go while
  nothing is watching, which is a scrollback the backend would have to keep.
- **A prompt's icons need a font this app can't ship.** Powerline separators
  and the git glyphs a modern prompt draws come from the private-use area,
  and the app's own mono has nothing there. The dock names the Nerd Fonts
  people actually install and falls back through them. A machine with any of
  them renders the prompt correctly. A machine with none shows those glyphs
  as empty boxes, with the text around them intact. Bundling one would be
  tens of megabytes for a decoration, and there's no setting to name a
  different font yet.
- **On Windows the dock holds a console window open for as long as it is open.**
  It's the ConPTY gap above, seen for longer. A release build has no console
  of its own, so a pty opened without the sideloaded runtime creates one. A
  tool call shows it for the length of a command; the dock shows it for the
  length of the session. The fix is the same: the two files the Windows
  bundle ships beside the executable. The startup log line saying whether
  they were found is the only warning you get.
- **A Mermaid fence draws two diagram types, and names the rest.** A
  ```` ```mermaid ```` block draws when it's a `flowchart`/`graph` or a
  `sequenceDiagram`. `classDiagram`, `stateDiagram`, `erDiagram`, `gantt`,
  `pie`, `mindmap`, `gitGraph` and the rest show as source, with a sentence
  naming what stopped them. That's the price of drawing these with the app's
  own two diagram engines instead of the Mermaid library (see
  [Notes](working-with-it.md#notes)), and it's the whole price.

  The library is 80 MB unpacked across d3, three cytoscape packages, katex
  and marked. It themes itself, loads its own fonts, and generates element ids
  per render that every screenshot would have to tolerate. The engines used
  instead already draw `show_flow` and `show_sequence`, in the app's palette,
  under tests that need no browser. If the named-and-refused list is what
  stops people using notes for diagrams, that's the evidence for drawing
  fences with the library instead of growing the reader.
- **The Mermaid library ships anyway, inside the sketch editor.** Excalidraw's
  own **Mermaid to Excalidraw** tool, which turns a diagram into shapes you
  can draw over, needs it and imports it lazily. It's 3.3 MB of the app's
  8.6 MB, measured by building with and without it, and it loads only if you
  use that tool. It can't be taken out cleanly. The tool's menu entry has no
  option to hide it and shares its only stable hook with the web-embed tool
  beside it, so removing the library would leave an entry that fails when
  chosen. A fence in a note is still drawn by the app's own engines. This
  changes the download size, not how a note looks.
- **Every Mermaid diagram is laid out left to right.** `graph TD`, `TB`, `BT`
  and `RL` are read and drawn as `LR`, with a line under the picture saying
  so. Direction in Mermaid is presentational: the same nodes, arrows and
  labels either way. So this re-orients without losing anything. But `TD` is
  the most common thing people type, and the picture isn't the shape they
  drew. Stages as rows would be a second geometry for the layout engine, with
  its own routing for all four kinds of edge, not a flag on the existing one.
- **Node shapes are read and then drawn as rectangles.** `a{Is it cached?}`
  and `a[(store)]` come out as boxes with the right text in them. The text
  carries the meaning, and it's parsed properly. The diamond that says "this
  is where it branches" isn't drawn, which is a real loss on a flowchart
  about a decision. Arrow *heads* are flattened the same way: `--o` and `--x`
  draw as ordinary arrows.
- **A `Note over` or a block frame in a sequence diagram is dropped.** The
  messages inside a `loop`, `alt`, `opt` or `par` still draw, in order. The
  frame around them and its label don't, and the diagram says how many it
  left out. Activation bars are ignored silently. `show_sequence` already
  holds that a picture somebody reads once carries two kinds of arrow, not
  four.
- **Notes are files, so two people editing one is git's problem.** A note
  saves itself and refuses to overwrite a version it hasn't seen. That covers
  the case this app can see: you and a running turn. Two checkouts, or two
  windows on the same folder, meet in the file and get reconciled the way any
  other file in the repository does.
- **A version kept for a note you left lasts as long as the window.** Say you
  leave a note while it shows two versions, or while its last save is refused
  or fails. Your version is kept in memory, and the note is marked in the
  list until you open it again. Close the window first and it's lost. Writing
  it to disk instead would make a second copy of every contested note,
  somewhere nobody looks, outliving the question it was kept for.
- **A note's diagram isn't searchable and its text isn't indexed.** Transcript
  search doesn't look in notes, and neither does the code index. A project
  note is a Markdown file in `.taurus/`, which the index skips along with the
  rest of that directory. You find a note by scanning the list, which is fine
  at a dozen and not at a hundred.
- **Chinese, Japanese and Korean text in a sketch isn't handwritten.**
  Excalidraw draws those scripts in Xiaolai, a 12 MB face. That's more than
  every other font, script and stylesheet in the app put together. It's left
  out, so that text draws in the system's own face instead. It still saves,
  exports and embeds.
- **A sketch's library lasts as long as the window.** Shapes you add to
  Excalidraw's library are kept in memory and not written anywhere.
  **Browse libraries** opens the public library site in your browser, and
  its **Add to Excalidraw** button returns to excalidraw.com, not to the app.
- **A sketch has no export.** Excalidraw's Save to disk, Open and Export image
  are turned off. The file is the sketch, and a second way to save it would
  bypass the rule that no save overwrites a version it hasn't seen. Copy as
  PNG from a selection's context menu is Excalidraw's own and still there.
- **Renaming a sketch leaves the notes that embed it pointing at the old
  name.** The embed says the sketch isn't there, and the fix is one line in
  the note. Rewriting other notes to follow the rename would mean the app
  editing prose somebody else wrote.
- **The model can't see a sketch.** There's no picture of one to give it, only
  the scene. What reaches it is the text written on each sketch a note
  embeds, through `read_note`. A sketch on screen with no note around it
  tells the model nothing, and there's no Ask about this on one.
- **A global note written by the model can't be rewound.** It's outside every
  workspace, so the checkpoint recorder has nothing to copy it into. The
  permission prompt shows the diff, and that's all the safety there is.
- **Excalidraw roughly quadruples the download.** Without sketches, the app
  measured 1.8 MB without source maps. With them it's 8.6 MB. None of it is
  in the chunk that starts the app, which grew by 5 KB. All of it is fetched
  from disk the first time you open a sketch, not before. Of the increase,
  3.3 MB is the Mermaid library above, 1.7 MB is Excalidraw's font
  subsetting, 1.2 MB is the editor, and 0.4 MB is its fonts. Its fifty-two
  unused translations are replaced with empty modules at build time.
- **The canvas holds one file at a time.** Opening another replaces it. Tabs
  would be a second navigation model to build and explain, and "open the
  readme while we talk about it" doesn't need one. The cost is comparing two
  files side by side, which is the terminal dock's job.
- **A canvas edit is invisible to the Changes panel and to rewind.** The
  drawer shows what this *conversation* changed and the way back from it.
  Your own typing is neither, so git is the undo that covers it. The cost is
  real: rewinding a turn restores the files that turn wrote. If you were also
  typing in one of them, your typing gets restored over. Nothing warns you.
- **A file changing outside a turn goes unnoticed.** The canvas reloads when
  the running turn writes the file (`files_changed` says so on the turn's own
  event stream) and when the document is opened again. A `git checkout` in
  the terminal dock, or another editor, is neither, so the canvas keeps
  showing what it read. Saving is still safe: it compares fingerprints and
  refuses instead of overwriting. But you find out when you save, not when
  the change happened. Watching the filesystem properly is a dependency and a
  lifetime to get right. Refreshing on window focus, the way
  `rescan_library` does, is the cheap version, and it isn't written.
- **The editor folds nothing, finds nothing, and has one cursor.** No code
  folding, no in-editor find-and-replace, no multiple cursors, no bracket
  matching. Each is a feature in its own right. The canvas shares its
  painted-textarea approach with the query box, so each would have to be
  built, not configured. That's the honest cost of not carrying a quarter of
  a megabyte of editor. The browser's own find-on-page still works, because
  underneath it's a real `<textarea>`. If this list becomes the reason people
  don't use it, that's the evidence for importing an editor instead of
  growing this one.
- **Source view doesn't wrap, so long prose lines scroll sideways.** A
  paragraph written as one long line runs off the right edge of the editor
  instead of folding. That's a decision, not an omission, and it was made by
  photographing the alternative. A wrapped line takes more than one row, and
  a gutter has one number per row. With wrapping on, the numbers walked off
  their own lines at the first long paragraph, with 4 pointing at the second
  half of line 3.

  A wrong line number is worse than a long line, because everything else here
  speaks in line numbers. The model points with them, the selection reports
  in them, and the chip on the composer repeats them. Markdown you want to
  read has the preview, which wraps and is the mode built for reading. Fixing
  it properly means measuring every line's height. That gives up both the
  windowed painting and the arithmetic that makes the caret cheap.
- **A query answers thirty rows, and that is a context limit rather than a
  reading one.** The model reads `query_data` results, and every row is paid
  for again on each later request of the turn. So the tool is shaped for
  aggregating. A query that wants a thousand rows should be writing a file.
  The pane has the same cap even though nothing there pays for context.
  That's the cost of one guarantee instead of two. The pane and the model go
  through the same call, and the alternative is a second limit that can be
  got wrong on its own. A result that hit the cap says so.
- **A refused query is refused by plan shape, not by intent.** `query_data`
  plans every statement and rejects it if the plan does anything but read.
  The match over plan kinds is written out in full, so a future DataFusion
  release that adds a writing statement fails to compile instead of being
  waved through. That doesn't cover a read that's merely *expensive*: a cross
  join over two million-row files is a legal SELECT. There's a 512 MB ceiling
  per query, so one of those fails instead of taking the app with it, and the
  tool is cancellable. But nothing estimates a query before running it.
- **A recipe transforms; nothing here judges.** A recipe is a chain of SQL
  steps, so it can clean, filter, join, derive, and rank: anything SQL can
  express. It can't do anything that needs a *model*, like classifying a free
  text column into a taxonomy, extracting fields from a description, or
  embedding a column for a recommender. For anyone building a dataset for an
  agent, those are the whole reason the feature exists, and none of them is
  written. They need a different shape from a SQL step, because a judgment
  over a million rows is a bill. It should be sampled first, reviewed, and
  only then committed to the whole file. Adding one as another `-- step:`
  would skip exactly the gate that makes it safe to run. See
  [Recipes](working-with-it.md#recipes).
- **What the Data pane was showing reaches the model but not the transcript.**
  A message sent from the pane carries the open dataset and the query box, so
  "this" has a referent. But it goes onto the prompt, not onto the
  transcript's copy of what was said. A `/command` expansion makes the same
  split, with the same consequence. A conversation reopened a week later
  shows "which category refunds most?" with no record of which dataset it
  meant. The chip above the composer makes it visible at the time, and the
  answer beneath usually names the dataset. That makes this bearable, not
  fine. Fixing it properly means a transcript entry that can carry more than
  text and images.
- **The turn strip says what's happening, not what was said.** It's one line
  above the composer while a turn runs in the Data pane, showing the running
  tool or the last sentence of prose. It isn't a transcript and can't be: a
  table or a chart has nowhere to go on one line. The question card matters
  most, because the turn parks and only you can unpark it. So it's called out
  instead of left to be inferred: the strip switches to a breathing mint ring
  and says *Waiting on your answer*. But you still have to give the answer in
  the conversation, and the strip can't show you the options.
- **The waveform's shape is the tool's category, which is coarser than the
  work.** Four shapes over six categories, and the categories are themselves
  a simplification. `grep` and `read_file` are both reads and draw the same
  sweep, though one is a search and the other isn't. Finer shapes would mean
  the harness classifying tools by something other than effect. Effect is
  what the categories exist to capture, and it's what the run header counts.
  The shape is a useful hint about the kind of work, not a readout.
- **Nothing says how long a turn has been running.** The motion says a turn is
  alive. It doesn't say whether *alive* means forty seconds or four minutes.
  The design's own working state pairs its waveform with an elapsed counter,
  which Taurus can't draw honestly. A tool call carries its own start time,
  but a turn doesn't, and a resumed conversation carries neither. A finished
  run reports its duration in the run header, which leaves exactly the case
  you'd want it for uncovered.
- **A query card stands alone, so a query-heavy turn is a stack of cards.**
  Any tool call that draws a view is left out of the folded run header.
  That's what stops a table getting filed under "6 steps · 11s" behind a
  disclosure triangle. `query_data` draws one, so a turn that asks six
  questions leaves six cards, not one row of six. Each is small and useful,
  but six in a column is more transcript than the turn is worth. The fix is a
  fold that can hold cards, which is a change to how a run is drawn, not to
  this tool.
- **Running a query from a card can disagree with the transcript above it.**
  The card carries the SQL and not the rows, on purpose: a remembered answer
  is the number that's right for a week and then quietly wrong. So
  **Run in Query** asks the files as they are *now*. If something has
  rewritten one since, the pane's answer and the model's sentence about it
  differ, with nothing saying why. That's the right way round, since the
  fresh number is the true one. But it's up to you to notice the
  disagreement.
- **A recipe's steps can't be taken to the query box.** The pane shows every
  step's SQL when you open a recipe, and there's deliberately no button to
  run one. Every step reads from `input`, the rows the step before it
  produced, and no such table exists outside a run. Pasting one into the box
  gets `table 'input' not found`, which would be the button's fault, not
  yours. Making it work means materializing the chain up to that step, which
  is most of a run. The sample-then-commit gate a model step needs would cover
  it too; see *A recipe transforms; nothing here judges*, above.
- **The drafts these buttons write are a guess at the question.** "Add this as
  a step in a recipe" doesn't say *which* recipe, because the pane doesn't
  know. So the model asks, or picks, and either way it's a round trip you
  could have saved by typing four words. The button is a head start, not a
  complete instruction. That's why nothing is sent and the cursor is left at
  the end of it.
- **The query box's highlighting is a scanner, not a grammar.** It knows
  where a literal starts and ends, which is the half a regex gets wrong. It
  knows nothing about scope. The function list is a fixed set of the common
  ones, so a DataFusion function nobody thought of, and any UDF, draws as a
  plain identifier instead of a call. Nothing is *wrong* on screen when that
  happens. A word is just the wrong color, which is the failure mode a
  scanner is chosen for.
- **Completion knows the files, not the query.** It offers columns that exist
  in a loaded dataset, and a CTE's output columns don't. `WITH t AS (SELECT
  a + b AS total …) SELECT | FROM t` won't offer `total`, because knowing it
  exists means planning the query, which is the engine's job and a round trip
  away. Aliases are found by sweeping for `FROM`/`JOIN` and the name after
  it. So the subquery form `FROM (SELECT …) t` isn't matched either, and `t.`
  falls back to offering every column in the workspace. Both cases degrade to
  a longer list, not a wrong one.
- **The caret the completion list hangs off is computed, not measured.** The
  box is monospace and doesn't wrap, so the list's position is arithmetic:
  one cell width, times the column, plus the padding. A character that isn't
  one cell wide (CJK, most emoji) puts the list a few characters off for the
  rest of that line. The alternative is measuring a mirror element on every
  keystroke, which is a lot of DOM for a case that doesn't come up in SQL.
- **A selection in the query box shows as a block of color with no text in
  it.** That comes from painting the query on a layer behind a transparent
  textarea. The browser draws the selection on the real control, whose text
  is invisible. The highlight is tinted harder than the app's default to
  compensate. Fixing it properly means an editor component, which is the
  dependency the whole arrangement exists to avoid.
- **The tables panel is reference and nothing is clickable in it.** That's
  deliberate, not unfinished. Completion is how text gets into the box, and a
  second insertion route would be a second set of rules about where the caret
  lands. It does mean you still have to type a column you read there, and
  the first three letters are all that costs.
- **Identifier case is Taurus's own dialect choice, and a recipe carries it.**
  DataFusion lowercases an unquoted identifier by default, the way Postgres
  does. Taurus turns that off, so a column reported as `Material` is written
  as `Material`. That's the right trade for data whose column names come from
  a spreadsheet header, because it makes the tool's own output valid input to
  itself. But a recipe written here is *stricter* than the same SQL pasted
  into a database client, where `SELECT MATERIAL` against a lowercase column
  would work. Nothing warns you about that when you copy a recipe out.
- **A recipe's SQL is DataFusion's SQL, and that goes into your
  repository.** The engine sits behind a trait so the rest of the harness
  doesn't name it. But a recipe is a file with SQL text in it, so swapping
  engines would leave every recipe anybody wrote in a dialect nothing reads.
  That's a known cost. The alternative is an invented step language, which
  buys portability nobody wants with unfamiliarity everybody pays for. It's
  also the reason exactly one method on the trait writes. Nothing *tells* you
  a recipe uses something dialect-specific. A `row_number() OVER` is portable
  and a DataFusion-only function isn't, and nothing distinguishes them.
- **A recipe writes one file, and there is no incremental run.** Every run
  reads the source from the beginning and rewrites the output whole. There's
  no "only the rows since last time", no partitioning, and no way to append.
  So a recipe over a growing export costs the whole export every time. Each
  intermediate step also spills to a scratch file. That keeps memory flat and
  the row counts exact, but a five-step recipe over a gigabyte does several
  gigabytes of temporary I/O. The scratch goes in the system temp directory,
  so a machine with a small `/tmp` is the case that fails. There's no setting
  to move it.
- **Running a recipe from the pane asks nothing first.** The button carries
  the path it writes, and that's the whole of the consent, the same as the
  query box. `run_recipe` called by the *model* does prompt, and its prompt
  names the path parsed from the same file the run will use. Neither offers a
  preview of the output before it lands. There's no dry run, no "this would
  drop 380,000 rows, continue", and the per-step deltas arrive after the file
  is already written. A rewind undoes it, which makes that acceptable, not
  fine.
- **Writing or editing a recipe isn't rewindable.** The checkpoint sweep skips
  `.taurus`, so a turn that authors a recipe leaves nothing for
  `taurus rewind` to put back. Project skills work the same way, for the same
  reason: these are the instructions, not the output. The file a recipe
  *writes* is fully rewindable. Git is the undo for the recipe itself. That's
  an argument for committing recipes, and not much comfort before the first
  commit.
- **The list of loaded datasets doesn't travel with the repository.** It lives
  in `~/.taurus/data/<workspace>/datasets.json`, beside the transcripts and
  the search index, not in the project's own `.taurus`. That keeps loading a
  file from being a *write* to the workspace. Otherwise looking at a CSV
  would cost a permission dialog, a diff in the Changes panel, and a line in
  the next commit. The cost is that a teammate who clones the repository has
  to load the files again, which is one sentence to the agent. A recipe
  sidesteps this by naming its own files (`source: data/events.csv`, plus a
  `tables:` block for anything it joins against). That's what lets a
  committed recipe run on a fresh clone. A recipe that names loaded datasets
  instead doesn't, and nothing warns you which kind you've written.
- **A profile is a full scan every time, and it can't be canceled.** Its
  result isn't cached. A dataset entry points at a file anything can rewrite,
  and a remembered profile is the kind of answer that's right for a week and
  then quietly wrong. So opening the pane on a multi-gigabyte file reads it
  again, and clicking away leaves that read running to completion. Caching it
  properly means invalidating on the file's length and modification time,
  which is how a page's row count is kept. Canceling means threading a token
  through the engine trait. Neither is written for the profile.

  What keeps this bearable is that a profile is the one thing that has to
  read everything. Loading reads a header, and paging counts a file once per
  version of it. A page deep into a CSV or NDJSON file still reads every row
  in front of it, because those formats have no index to seek by. Measured on
  an 80 MB file of two million rows: 14.1 ms for the first page and 16.3 ms
  for one at row 1,999,900, against 14.4 and 29.0 ms with every page counting
  the whole file again.
- **`.json` means newline-delimited JSON, not a JSON array.** A file holding
  one big `[ {...}, {...} ]` is refused with a message instead of read.
  Reading it would mean parsing the whole thing into memory before any of the
  streaming downstream could start, and that's the one shape of file this is
  meant to protect you from. Converting it is one `jq` line, and the agent
  can run it. There's no Excel reader either, and adding one is a dependency,
  not a design question.
- **A dataset has to sit on a local drive.** A file reached by its UNC name,
  like `\\fileserver\reports\q3.csv` or anything under a Windows share that
  hasn't been mapped to a drive letter, is refused when you load it. The
  message says to copy it into the workspace first. The engine addresses
  files by `file://` URL, a share becomes a URL with a *host* in it, and only
  the local filesystem is registered to serve one. Mapping the share to a
  drive letter is the workaround, and it's a real one. Serving
  `file://server/` properly means registering a second object store per
  share, which is more machinery than the case has asked for so far.
- **A nested column is counted and not described.** A list, a struct, or a
  map profiles as how many rows have one, and nothing else: no distinct
  count, no range, no common values. None of those has an answer until the
  column is flattened. The column is kept, not refused, so one nested column
  in an export doesn't cost you the other thirteen. Flattening is a
  transformation. A recipe can flatten one with an `unnest` step, and until
  somebody does, the profile says what it can.
- **The grid doesn't sort or filter.** It pages, a hundred rows at a time, in
  the order the file is in. Sorting a million rows is a query, not a click.
  It has to go back to the engine, and the pane would need somewhere to say
  it's running one. Filtering is the same thing with a predicate. Both are
  worth having. The transcript's `show_table` sorts because its rows are
  already in the browser. These aren't, and pretending otherwise would sort
  the hundred rows on screen and call it sorted.
- **Searching a conversation reads every transcript, every time.** There's no
  index. What makes that affordable is the file's shape, not a structure kept
  beside it. A transcript is JSONL, so if its bytes don't hold the query, it
  can't hold it once parsed either. A conversation that doesn't match costs
  one read and nothing else. Measured on sixty-one real conversations across
  every workspace, a whole-history search is about 110ms. That's why the
  palette debounces instead of searching per keystroke, and why its two local
  groups answer first while this one fills in underneath. It grows linearly
  with how much you've said. An index would need a rule for when to rebuild
  it, and a stale index that quietly stops finding last Tuesday is worse
  than a search that takes a tenth of a second. See
  [Finding a conversation](working-with-it.md#finding-a-conversation).
- **The search is literal, and it doesn't read tool calls.** No regex, no
  fuzzy matching, no stemming: `banner` doesn't find `banners`. It reads
  prose only: what you typed and what the model wrote back, not a tool's
  arguments or its results. That last part is a decision, not an omission,
  and it's what makes the results usable. Tool results are file contents and
  build logs, so including them would match nearly every conversation for
  nearly every query. The cost is real, though. Something that only ever
  appeared in a file the agent read isn't findable here. For that, `grep`
  over `~/.taurus/sessions` is the honest answer.
- **A search hit is found again by text, not by position.** The search
  reports which message matched. The app throws that away and looks for the
  words again in what's on screen. The two don't count the same things (a
  turn folds a prompt, an answer and a run of tool calls into one card), and
  looking again is simpler and also right for a conversation compacted since.
  The cost is the case where the hit was summarized away: the conversation
  opens, nothing is marked, and nothing says why. It also marks the *first*
  turn holding the words, not the one the search found. The two differ when a
  conversation says the same thing twice.
- **Coloring code is a scanner, not a parser.** One walk serves every
  language, parameterized by how a comment opens, which delimiters quote a
  string, and which words are the vocabulary. That's enough to be right about
  ordinary code, but not about all of it. A construct it misreads is colored
  wrongly instead of reported, because nothing here could report it. It
  knows Rust, TypeScript and JavaScript, Python, Go, shell, SQL, JSON, YAML,
  and TOML. Everything else renders plain with its label intact. That
  includes HTML and CSS, which are common in a fenced block and whose syntax
  isn't word-shaped. Growing the list is a `Grammar` each. Growing it to
  *markup* is a second scanner, because a word-oriented one turned loose on
  HTML produces confident nonsense.
- **An intra-line diff mark is a trim, not a diff.** The common words at each
  end of a replaced line come off, and whatever's left in the middle is
  marked. That's one region per line by construction. So a line with two
  separate small edits is marked from the first to the last, including the
  unchanged text between them. Two cases decline outright instead of
  guessing. One is a line rewritten end to end, where marking almost all of
  it would look like a finding. The other is a run of removals answered by a
  run of additions of a different length, where pairing by position would
  mark the difference between unrelated lines. In all three cases the
  line-level `+` and `−` are still exactly right, which is why declining is
  affordable.
- **What a tool cost in the Context panel is apportioned, not measured.** The
  provider reports one number for a whole request and never says which part
  of the prompt was whose. So every figure there except the billed row is the
  harness's own four-characters-a-token estimate. It's the same estimate the
  compaction threshold runs on, with the same limits. It's accurate enough to
  rank tools against each other, which is what the panel is for, but it isn't
  a bill. The two exact numbers are what the provider reported in and out.
  See [The context window](working-with-it.md#the-context-window).
- **The panel accounts for tokens, not money.** No provider's prices are in
  here and none are fetched, so nothing multiplies the billed tokens by a
  rate. Adding it means a price table per provider per model that somebody
  has to keep current. A table six months stale reporting dollars to two
  decimal places is worse than no dollars at all.
- **There are five window shortcuts and two more inside one dialog.** ⌘K and
  ⌘⇧P open the palette, ⌘N starts a conversation, ⌘L puts the cursor in the
  composer, ⌘, opens Settings, and ⌃` shows the terminal. That's the whole
  window list, and it's short on purpose. Every one is also a palette row
  showing the key it answers to, so the palette is where you discover them.
  Adding a sixth is cheap in a way adding the first wasn't.

  The permission dialog is the exception, and it's argued for, not an
  oversight. ⌘↵ allows once and ⌘⌫ denies, printed on the buttons. The window
  list stays short because it's a shared namespace. A modal has none, so a
  chord bound there collides with nothing and needs no discovery beyond the
  key on the button it fires. It's also the most-pressed control in the app.
  See [Permissions](safety.md#permissions) for why the two standing grants
  get no key.

  There are no user-defined bindings. A keymap file means a conflict
  resolver, a way to see what's bound, and a way to find out why a key did
  nothing.
- **One message can be typed ahead, not a list of them.** Press Enter during a
  turn and the message is held and sent when that turn finishes. A second one
  typed ahead replaces the first instead of joining a queue. That's a
  decision. The composer is one box, with no way to see three pending
  messages, reorder them, or edit the second one. A queue would be state the
  app holds and you can't inspect. The cost is that a real list of
  follow-ups has to be sent one at a time.
- **"Edit" on a question re-asks it; it does not rewind the conversation.**
  The button puts a sent message back in the composer. The original stays in
  the transcript, along with whatever it produced. A true edit would mean
  truncating the conversation at that turn: dropping the later messages from
  the transcript on disk and from the model's next request. That's a
  different feature with a different failure mode, where a mis-click
  discards an hour of work. The files a discarded turn wrote are already
  recoverable through [Rewinding a turn](safety.md#rewinding-a-turn); the
  words aren't.
- **A "try again" is a resend, not a resume.** The harness can't pick a turn
  up partway, so retrying a turn that died runs it from the top. If the turn
  had already written files before it broke, those writes stay. Both turns
  are in the checkpoint log and either can be rewound. That's the honest
  arrangement, not the convenient one: folding them together would leave a
  rewind that undoes twice as much as its label says.
- **The dock badge is not on Windows.** The desktop is told when a turn needs
  somebody: a badge counting what's owed, and a bounce when the count rises
  while the window isn't focused. `set_badge_count` is unsupported on
  Windows. The documented substitute there is a taskbar overlay icon, which
  means shipping a rendered image per count. So the taskbar flash carries the
  signal there, and it works. On Linux the badge needs a desktop with
  `libunity` and is dropped where there isn't one. Neither fallback is silent
  to the *user*, who still gets the flash. Both are silent to anyone reading
  the code and expecting a number.
- **Changes and the canvas share one column, so opening one hides the other.**
  Both dock to the right of the conversation, and only one is drawn at a
  time. The canvas is hidden, not closed, so its unsaved typing and scroll
  position come back when you shut Changes. This refuses a third column. At
  the width the window is designed for, the transcript already gives up half
  of itself to whichever one is open, and splitting the rest three ways
  leaves nothing readable. So reading a diff next to the file it changed
  means closing one of them.
- **Chunking for the index is a line window, and structure-aware chunking was
  tried and lost.** Forty lines with ten of overlap, in every language. The
  obvious improvement is to cut where a definition starts. The obvious
  objection is a grammar per language, a silent fallback for the ones you
  lack, and confident nonsense on a file half understood. That objection
  doesn't apply to reading *layout*, though. In every language a person
  writes by hand, a non-blank line at zero indent, after a blank line or
  after the close of what came before, starts a new top-level thing. Snapping
  a cut to the nearest one within twelve lines needs no grammar and has no
  second code path.

  It was built that way, and measured. On this repository, with fifteen
  questions and `nomic-embed-text`, line windows scored MRR 0.668 and put the
  answering file first 53% of the time. Structure-snapped cuts scored 0.598
  and 40%. Adding an embedded heading (the file's path and the definitions
  the chunk sits inside) scored 0.565 and 40%. Restoring the overlap that
  snapping drops recovered nothing (0.577), which rules out the obvious
  confound. The numbers are deterministic and reproduced exactly across runs.

  So it isn't shipped. If anybody wants to try again, `git show` on the
  commit before the one that reverted it is the implementation.

  What the measurement does **not** settle: fifteen questions is a small
  sample, one embedding model is one embedding model, and the corpus is Rust
  and TypeScript. A model that reads code structurally, or a workspace in a
  language where indentation carries more, could land differently. One thing
  it didn't isolate: snapping produces 13% fewer passages (4025 against
  4610). A corpus with more passages gives every file more chances to be the
  best match for something. The overlap control lengthened chunks instead of
  adding them, so that axis is untested.

  `cargo run -p taurus-index --example retrieval` is the gate, and re-running
  it is all it costs to argue with any of this. It has to be run the way that
  comparison was, though: scoring both things against one corpus in one
  process. The corpus is the working tree, and editing a doc page between two
  runs was measured moving MRR by 0.03. That's the size of the differences it
  exists to detect.
- **Reranking is off by default, and ungated.** `rerank_model` is empty
  because the plan that added it said to beat cosine before turning it on by
  default, and that comparison hasn't been run. There's somewhere to run it.
  The retrieval harness above scores whatever the index currently does, so
  the gate is one command with the setting on and one with it off. Until
  somebody does that, an empty default is the honest state, not a forgotten
  one.
- **A theme sets fourteen colors, three typefaces, a wordmark and a corner
  radius, and nothing else.** That's not a stub; the ceiling is the point.
  Everything below the top of `src/styles.css` speaks in roles, so those
  fourteen values move the whole window. If a theme could restate a *rule*
  instead, it could break a layout in a way only its author could reproduce.
  "The app is broken" would then be a report nobody could tie back to a
  color picker. The spacing ladder isn't exposed, for the same reason. It's
  a constraint the stylesheet's own tests enforce, and a theme that could
  redefine it could make the app look like nobody measured anything. The
  cost is real: a theme can't change a font size, a weight, a shadow, or the
  width of the rail.
- **A theme can't bring a typeface with it.** `fonts` names families, and
  they have to be installed on the machine already. The window's CSP allows
  no remote stylesheet, so there's nothing to point a `@font-face` at.
  Bundling a font file inside a theme would mean reading arbitrary binaries
  out of a config directory and injecting them as `data:` URIs, which is a
  wider hole than the feature is worth. A theme naming a font nobody has
  falls back to the stack the app ships instead of failing. So this is
  quiet, not broken, and quiet is its own problem: nothing on screen says the
  font wasn't found.
- **Themes are read fresh on every status, and only the active one carries its
  logo.** The picker's full scan happens when the picker opens. The split
  exists because a resolved theme carries its logo inlined as base64, and
  the status is pushed after anything that moves a number on screen. A scan
  on that path would re-encode every logo on the machine several times a
  turn. The cost is that problems in a *different* theme, in a file you
  aren't using, aren't reported until you open Settings › Appearance.
- **Ending a child reaches its whole tree — everything that stayed inside
  it.** A hook that hits its timeout and a background command that's Stopped
  are both usually a shell. Killing the child alone would reach `/bin/sh` or
  `cmd.exe` and leave the linter, the build or the watcher running while the
  app reported it stopped. So each starts as a tree (`taurus_process::Tree`).

  On Unix that's a process group of its own, ended with
  `kill -KILL -- -<pgid>`. On Windows it's a Job Object: the child starts
  suspended and goes into the job before it runs, and every process it starts
  after that is in the job, however its parents come and go. Both are tested
  against a real tree on every platform in CI. That includes a grandchild
  whose parent has already exited, the shape an npm `.cmd` shim leaves every
  time. The cost on Unix is a fork of `kill`, and only on a path where
  something has already hung or been stopped by hand. On Windows it's a
  snapshot of the system's threads each time a hook or background command
  starts, to let the suspended child go.

  The Unix spelling is a scar twice over. Without the `--`, procps reads
  `-123` as a signal, not a pid, and signals *the caller's* group: eleven of
  twelve kills on Ubuntu left the tree running and killed Taurus instead. And
  a pid past `i32::MAX` negates to `-1`, which on Linux means every process
  the user owns. It took out three CI runners from inside a test written to
  prove the opposite.

  A kill that can't be carried out says so on the hook's own refusal, not in
  a log nobody reads. What escapes is a process that leaves on purpose. On
  Unix that's one that starts a session or group of its own (`setsid`, a
  daemon's double fork). On Windows it's anything started outside the tree
  on the child's behalf, such as a scheduled task or a service. Nothing short
  of a container catches those, and a hook or command that does it means to
  leave something running.
- **Two other children are killed one process at a time.** A *foreground*
  `run_command` stays in the parent's process group on purpose. That way a
  terminal's own Ctrl-C reaches the whole tree without anything in this code
  having to run first, which is worth more than a group would be. So its
  timeout kills the shell alone. And a skill's script
  (`taurus_skills::tools`) uses `kill_on_drop` with no tree kill at all. Both
  would be the same small change as above, wherever it's wanted. Neither has
  been made, because neither has the safety argument the hook timeout has: a
  hook is a guard whose whole promise is that it stopped something.
