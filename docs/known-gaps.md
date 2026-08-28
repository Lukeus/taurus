# Known gaps

<sub>[← Taurus AI Shell](../README.md)</sub>

What Taurus does not do, stated where it can be read before it is discovered.
Each entry says what is missing, and what covering it would cost.

This list grows, and that is not the same as debt accumulating. An entry arrives
when a feature ships and someone writes down where it stops, so a longer list
is mostly a sign of more features honestly described. Most of what is here is
permanent by construction and says so in its own words — a terminal has one
output stream, `fetch_url` runs no JavaScript, nothing can make a model write a
note. Read those as documentation. The ones worth watching are the entries that
end by naming a specific thing that has not been built; those are the backlog,
and they are the minority.

- **A pty on Windows depends on a runtime fetched at build time.** The Windows
  bundle ships Microsoft's redistributable ConPTY beside the executable —
  `conpty.dll` and a headless `OpenConsole.exe` — because the system's own
  console host shows a window when the process asking for it has none, which a
  release build does not. `portable-pty` prefers a sideloaded `conpty.dll`, so
  those two files are the whole fix. What it costs: 2.2 MB in the installer, a
  network fetch during a Windows build, and a version pinned by hash that
  somebody has to bump. A machine that ends up without them still runs every
  command — the pty falls back to pipes and the result says so — but the
  fallback loses the terminal behaviour the call asked for. The app logs at
  startup whether it found the runtime, which is the only signal available:
  packaged wrongly, everything works except for a window that only a user on
  Windows in an installed build can see. See
  [Running commands](safety.md#running-commands).
- **A background command's tab is polled, not pushed.** The dock holds one
  per command — see [Terminal](capabilities.md#terminal) — and it asks four
  times a second while a tab is on screen rather than being told when a line
  arrives. That is a decision and not a stub: the alternative is a subscription
  per job with a lifetime to get right at both ends, over a buffer that is the
  record anyway, where a missed message would cost nothing a later read does not
  repair. What it does cost is a quarter second of latency on a line, and one
  IPC call every two seconds while any command is still running. A window with
  nothing running makes none.
- **What a background command printed is capped at 256 KB.** The buffer is the
  whole record: it is what the tab draws from as well as what `check_command`
  reads, so a build that printed more than that has lost its beginning from both
  — said in the pane where the gap is, rather than skipped over. A long test run
  is comfortably inside it; `cargo build -vv` on a cold cache is not. Raising it
  is a number, and the reason it is not higher is that this is held per command
  for as long as the workspace is open, times eight.
- **A background command's tab cannot be typed into.** It is text and not a
  terminal, which follows from the gap below: there is no pseudo-terminal behind
  one of these, so there is nothing to type into and nothing drawing a screen. A
  program that stops to ask a question cannot be answered, and the only thing to
  do with it is stop it.
- **A background command has no pseudo-terminal.** `pty: true` and
  `background: true` together are refused rather than silently doing one of
  them. The pty path runs a command to completion behind a blocking read, and
  handing back a handle to one instead means a second implementation of the
  drain and the stop, per platform. So a dev server that colours its output
  loses the colour, and a program that refuses to start outside a terminal
  cannot be backgrounded at all — it has to be run in the foreground, where the
  timeout applies again.
- **A command still running when a turn ends is in no turn's changed-file
  list.** Its pre-image is held from the moment it started and spent when it
  exits, so nothing is lost — the changes land in whichever turn is running
  when it finishes. But a rewind offered while a build is still writing cannot
  include what the build has not written yet, and the list the user reads
  before deciding says nothing about the command that is about to add to it.
  Covering it means the changed-file list growing under the reader's eye, which
  is a UI question rather than a recording one.
- **What a message costs is still estimated, at four characters a token.** The
  fixed part of a request is measured — a response reports the whole prompt's
  size, and the difference from the estimate for the same messages is the
  system prompt, the tools, and the envelope, exact. What is not measured is
  the drift *inside* the messages: a tokenizer that makes 3.2 characters of a
  token out of minified JSON leaves the estimate low by a fifth on a
  conversation full of it, and the overhead cannot absorb that because it
  grows with the messages rather than sitting beside them. Closing it means
  either a tokenizer per model in the harness — the thing that would have to
  be kept in step with every backend forever — or a count-tokens round trip
  before each request, which is the cost the estimate exists to avoid. The
  threshold is what covers it, and the meter above the composer is what makes
  being wrong visible.
- **A hook can refuse a tool call and cannot approve one.** There is no
  `allow` verdict, so a hook cannot skip a permission prompt the way one in some
  other harnesses can. That rules out the "approve every `git status` for me"
  use, and the trade is deliberate: a hook that could approve makes `hooks.json`
  a second permission surface, one that has to be trusted exactly as much as
  `permissions.json` and kept in step with it. As it stands, adding hooks to a
  machine can only ever shrink what it will do, which is what lets a project's
  hook file be honoured at all. The narrowing use — "never let it force-push" —
  is the one this covers. See [Hooks](configuration.md#hooks).
- **A hook that cannot run blocks the call.** A missing program, a crash, or a
  timeout denies on the two events where there is still something to deny. So a
  typo in `hooks.json` stops every call it matches until it is fixed. That is
  the intended direction rather than an oversight — a guard that treats its own
  breakage as approval has stopped guarding at the one moment it mattered — but
  it does mean a hook is a thing that can break a working setup, which a purely
  observational one could not. `taurus hooks check` names the entry and the
  field.
- **Hooks are not told about a delegate's turn boundaries.** A sub-agent's tool
  calls go through the same `pre_tool_use` and `post_tool_use` hooks the
  parent's do — the context is shared, which is what stops a delegate routing
  around a guard. `user_prompt_submit` and `stop` fire for the conversation, not
  once per child: a delegation is one tool call from the outside, and firing
  "the turn ended" four times for one turn would make a `stop` hook that counts
  anything wrong. Covering it properly means an event pair of its own, and
  nothing yet needs one.
- **Trust is per folder, and it is answered once.** A workspace you have
  vouched for stays vouched for, so a `git pull` that adds a server to
  `.taurus/mcp.json` is read on the next turn without asking again. Fixing that
  means fingerprinting the config and re-asking whenever it moves, which sounds
  strictly better and is not: the file changes on ordinary branch switches, and
  a prompt that appears on most `git checkout`s is a prompt that gets clicked
  through — including the one time it mattered. The decision on offer is
  therefore "this project may configure Taurus", the same unit an editor's
  workspace trust uses, and the honest reading is that it is trust in the
  project's maintainers rather than in a particular revision. `taurus trust
  --revoke` and the Settings row are what withdraw it. See
  [Trusting a workspace](safety.md#trusting-a-workspace).
- **Trusting a workspace is about its config, not about its code.** The gate
  decides whether a folder's `.taurus` may configure the harness. It says
  nothing about what running that project's build script does, and it cannot:
  once you ask an agent to work in a repository, the repository's own code is
  the thing you asked it to run. Commands still go through the permission
  prompt, which is where that decision is actually made. An untrusted workspace
  is not a sandbox and is not described as one.
- **Nothing re-reads trust between turns on its own.** The desktop app asks the
  backend for it when it refreshes, and the CLI once per command. A workspace
  that gains its first `.taurus/mcp.json` while the window is open raises its
  banner at the next refresh rather than the moment the file lands — the same
  turn-boundary rule the rest of the config follows, and for the same reason
  there is no watcher here. See
  [Trusting a workspace](configuration.md#trusting-a-workspace).
- **A rewind does not cover ignored directories.** A file an ignore rule
  excludes by name is covered; everything under a directory an ignore rule
  excludes is not, so a command that rewrites something in `target/` or
  `node_modules/` is neither listed nor restorable. Widening it means indexing
  those before every command, which is not affordable, and having a rewind
  delete build output, which is not wanted. See
  [Rewinding a turn](safety.md#rewinding-a-turn).
- **A rewind reports git state, it does not put it back.** `.git` is left out
  of the walk, so undoing a turn that ran `git checkout` or `git reset --hard`
  restores the file contents while leaving `HEAD` and the index where the
  command moved them — a tree that matches neither commit. Covering it properly
  means snapshotting the object store, which is its own feature and not one this
  opens. What has been closed is *when you hear about it*: the sweep writes the
  fact into the checkpoint log, and the rewind plan repeats it beside the file
  list, so the warning arrives at the moment you reach for undo rather than
  only at the moment the command ran. Staging is still unreported — see
  [Rewinding a turn](safety.md#rewinding-a-turn) for why the index is deliberately not
  watched.
- **A change that moves neither length nor timestamp is invisible.** The same
  walk compares size and modification time, which is what `make` and `rsync`
  have always compared. On a filesystem with nanosecond timestamps defeating it
  takes deliberate effort; on one with coarse timestamps, a command that
  rewrites a file to the same length within the same tick would slip through.
  Closing it means reading every file twice per command.

  The commands after the first in a turn reuse what the previous one read,
  keyed on that same length and modification time, so a workspace is read once
  per turn rather than once per command. That is the same comparison and so the
  same blind spot, but it reaches one case further. Where a sweep on its own
  would merely fail to *notice* an invisible change, a reused read can also
  carry the wrong pre-image: if a file is rewritten to the same length and
  timestamp between two commands, and a later command in the same turn changes
  it visibly, what a rewind puts back is the version from before the invisible
  edit. It is bounded on both ends — reaching it needs a deliberate
  same-length, same-tick rewrite in the window between two commands of one
  turn, and a file the turn already recorded is unaffected, because the first
  pre-image of a turn is the one that is kept. Closing it is the same read
  every file twice, in the same place.
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
- **An instructions file is read at turn boundaries, not watched.** `AGENTS.md`
  and everything it imports are re-read at the start of each turn, so an edit
  lands on the next message rather than the next reload. It is still not
  *watched*, and that stays: a watcher fires whenever an editor happens to save,
  which is routinely the middle of a running turn, and the brief a turn was
  given has to be the one it started with. What is left is the one-turn delay
  and one narrow blind spot — the freshness check compares length and
  modification time, so a rewrite to the same length within one filesystem tick
  waits for the next change. That is the same comparison the sweep makes, and it
  is closed the same way: by reading every file on every message, which is what
  the check exists to avoid. See
  [Instructions](capabilities.md#instructions).
- **Warnings are matched by their headline, which is not the same as by their
  lint.** Two hundred sites of one clippy lint print the same headline and
  collapse to one body; two hundred `unused variable` warnings name a different
  variable each time, so each is its own headline and none of them collapse.
  That is the weaker half of the same feature, and it is a limit of the text
  rather than of the code: rustc prints the `#[warn(...)]` note that would name
  the lint only on a lint's first occurrence. What would close it is
  `--message-format=json`, which the model would have to have asked for — and
  rewriting a command line to add a flag is a different thing from shortening
  its output, with a different failure mode: the command that ran would no
  longer be the command that was approved. See
  [Output too big to hand over](safety.md#output-too-big-to-hand-over).
- **The test filter knows libtest and nothing else.** It recognizes the format
  `cargo test` prints, which covers a Rust workspace and a directly-run test
  binary. `cargo nextest`, `jest`, `pytest` and `go test` all announce
  themselves differently, so their output is collapsed only where it repeats
  itself, which for a passing suite is not at all. Each is a small filter of
  its own beside the existing one; what stops them being written today is that
  none of them can be checked against real output from this machine, and a
  pattern nobody has run against the thing it matches is a guess. See
  [Output too big to hand over](safety.md#output-too-big-to-hand-over).
- **Only repetition that is literally consecutive is collapsed.** A command
  large enough to be worth the pass — 16 KB on a 200,000-token model, and a
  share of the window on any other — that prints the same line three or more
  times in a row keeps one copy and a count, which is most of what a chatty build or a retrying server
  produces. It is not most of what a *server* produces: two messages
  alternating — a retry and the timestamped line after it — are never
  adjacent, so nothing collapses and the stream is as long as it was. Closing
  that means matching lines that are merely similar, which is a different kind
  of claim: a count of identical lines is a fact, and a count of lines that
  looked alike is a judgement the model cannot check. The other half is that
  none of this applies to `check_command`. A background command's readers keep
  byte cursors into a shared buffer, and shortening the text those cursors
  count would move the model's place and the window's apart. See
  [Output too big to hand over](safety.md#output-too-big-to-hand-over).
- **A cut command's output can be swept away while the transcript still points
  at it.** When a stream runs past what the model's window has room for the
  whole of it is written to
  `~/.taurus/output/<workspace-key>/` and the gap in the result names the file,
  so the middle of a long build is a `read_file` away rather than a re-run. But
  twenty streams are kept per workspace and the oldest go as new ones arrive, so
  a message from earlier in a long session can name a file that a later command
  has since displaced — and reopening a saved conversation weeks later will
  usually find nothing there at all. The model is told the file is missing
  rather than shown the wrong one, which is the important half. Closing the rest
  means either keeping build logs indefinitely, or pruning against the
  transcripts that reference them, which is a second index of the kind
  [Sessions](../crates/taurus-host/src/sessions.rs) deliberately does not keep.
  Size is not the other half of it: `read_file` windows around the offset it is
  given, so a spilled stream opens at any line however large the file. See
  [Output too big to hand over](safety.md#output-too-big-to-hand-over).
- **A diff is shown for `write_file` and `edit_file` and nothing else.** A
  command line has no before-and-after to compute, which is exactly why
  `run_command` is swept afterwards rather than predicted. So the most
  consequential writes in a session — the ones a script made — are still
  approved on the command line alone. They are at least *readable* afterwards:
  the **Changes** drawer diffs what the sweep recorded, so a `sed -i` across a
  dozen files can be inspected line by line once it has happened. That is
  review after the fact, not before it.
- **A turn's diff attributes a hand edit to the wrong turn.** What a turn
  changed is computed as its own pre-image against the next recorded pre-image
  of the same file, which is exact for anything Taurus did and silently wrong
  for anything you did in between — your edit appears inside the later turn's
  diff. Closing it means post-images, which is a second copy of every file
  written per turn to attribute a case the rewind already warns about in the
  same words. See [Keeping a turn](safety.md#keeping-a-turn).
- **Committing a turn still does not offer to squash.** The checkpoint log
  records which turns are in `HEAD`, so committing turn 5 while turn 4 is
  uncommitted says so first — and says something sharper when the two share a
  file, because `git commit -- <paths>` takes what those paths hold now and
  would carry turn 4's edits in wearing turn 5's message. What is still missing
  is the other half: nothing offers to commit a run of turns as one. That is not
  a record shape any more, it is a second commit path with its own message
  editor and its own failure modes, and it has not been built. See
  [Committing a turn](safety.md#committing-a-turn).
- **A committed turn can still be rewound, deliberately.** Rewinding past a turn
  you committed restores the files and leaves the commit in place, so the tree
  no longer matches it. That is still true and is not going to change — it is
  your tree, and there are good reasons to want the files back regardless. What
  the rewind plan does do is name the commit and what to do about it (`git
  revert`, `git reset`) before you press anything, rather than letting you find
  out afterwards. The one thing it will not do is refuse: a rewind that
  second-guessed you would be a worse tool than one that tells you.
- **Nothing makes a model write a note either.** `remember` is offered and the
  prompt says when to reach for it, and that is the end of the harness's
  leverage — the same limit `update_plan` has, for the same reason. A model that
  finishes a long piece of work and simply answers leaves nothing behind, and
  the next conversation starts as blank as it would have before the feature
  existed. Forcing one at the end of every turn would spend an iteration on the
  many turns with nothing worth carrying, and would fill the drawer with notes
  about turns nobody needed a note about.
- **A note is not checked against the workspace it describes.** It says what was
  true when it was written, and nothing revisits it — a note about a branch that
  has since merged, or a file that has since been deleted, is carried into every
  later conversation exactly as confidently as one still true. The prompt says
  so in as many words, which puts the judgement on the model where it has to be,
  and the Memory drawer is where a stale one gets removed. Expiring them
  automatically would need a model of what each note is *about*, which is a
  larger claim than a line of prose supports.
- **Nothing makes a model plan.** `update_plan` is offered and the prompt says
  when to reach for it, and that is the end of the harness's leverage. On a
  five-step mechanical task neither `qwen3.6:27b` nor `qwen3.5:9b` called it
  unprompted — both simply did the work — so the feature earns its keep on the
  long, exploratory turns where drift actually happens, and on the models that
  take the instruction. Forcing a plan on every multi-step request would spend
  an iteration and a card on turns that never needed one.
- **A plan can still end in progress — but not silently.** A model that finishes
  the work and goes straight to its answer leaves a step reading `[>]` that is
  actually complete, and the pinned panel keeps that stale version somewhere you
  cannot miss. The harness cannot decide a plan is finished, so it does the one
  thing it can: when the model tries to end the turn with steps open, it is
  asked once to send the list back closed, and the turn continues. That is the
  same lever as the verify nudge and it has the same limit — a model that
  answers the question without calling `update_plan` gets to stop, and the panel
  then clears on the next request as before.
- **A plan does not survive the process.** The board is held per session in
  memory, not written to disk, so an unfinished plan carries across messages and
  is gone if the app restarts. Reopening the conversation redraws the panel from
  the transcript, because that is derived; the model's copy has to be rebuilt by
  the model. Persisting it means a second record that can disagree with the tool
  calls that made it, which is a worse failure than re-deriving.
- **Whether a carried plan still applies is the model's call.** The harness
  clears a finished plan and labels an unfinished one as belonging to an earlier
  message. It cannot tell whether a follow-up continues the task or changes the
  subject, so a model that ignores the label works the old checklist against the
  new request.
- **A branch is warned about, not enforced.** A conversation started on `feat/x`
  and resumed on `main` is labelled in the rail, each of its turns records the
  branch it began on, and a rewind that would write those pre-images over a
  different tree says so beside the plan. It still does not refuse, and its file
  references still point where they pointed. Refusing is the version that has
  not been built and probably should not be: a rewind onto another branch is
  occasionally exactly what someone means, and the warning is what separates
  that from the accident.
- **A sub-agent's answer is summarized, not streamed.** Its tool calls appear
  under the delegation card as it makes them, so a long delegation looks alive
  rather than hung, but its reasoning and prose stay inside the child. That part
  is deliberate: the parent asked for a conclusion, and a second conversation
  inlined into the transcript is what delegation exists to avoid. What is *not*
  deliberate is having nowhere to read it afterwards, and that part is covered
  — every delegate keeps its own transcript beside its parent's, written as it
  runs, and the delegation card opens it in a drawer while the call is still
  running or long after it finished. `taurus sessions --agents <ID>` lists them
  for the CLI, which prints where they are rather than rendering one: a
  conversation inside a conversation is the thing the terminal has no second
  pane for. What a *resumed* conversation loses is the link, not the
  transcripts: the parent's own record says a delegation happened, not where
  its child was written, so a reopened conversation's cards no longer offer to
  open one. The files are still there, and `--agents` still lists them.
- **A custom agent's roster is frozen for the turn, on purpose.** The set of
  sub-agents is snapshotted when a turn starts, so an agent file saved mid-turn
  is not visible until the next one. That is the whole of it: the
  directories are checked at every turn boundary and rescanned when anything in
  them moved, so a new agent is available on the next message rather than after
  a reload or a trip to the drawer. The remaining freeze is the feature — a turn
  must delegate against the roster it started with — and it is why there is no
  file watcher here rather than an admission that one is missing. The
  same-length-same-tick blind spot above applies to the check here too. One
  surface still lags on purpose: the `/` command *menu* lists the last scan,
  because it redraws on every keystroke and taking config locks there is how a
  reload deadlocks against typing — typing the name in full works immediately,
  and the menu re-reads itself whenever a rescan changes how many skills or
  agents there are, which is a message finishing, a drawer opening, or coming
  back to the window. See
  [Sub-agents](capabilities.md#sub-agents).
- **A proposed agent's system prompt is reviewed by eye, and nothing else.**
  `propose_agent` checks the shape — the name, the description, the tool scope,
  whether it duplicates an existing agent — but the prompt itself is prose, and
  prose that will steer a delegate on every future turn. That is the same
  exposure `propose_skill` has always had, and it has the same answer: the card
  shows it in full, unelided and editable, and nothing is written until you
  approve it. There is no check that reads what it says. See
  [Sub-agents](capabilities.md#sub-agents).
- **Reading another client's directories is not the same as being that client.**
  Taurus reads Claude's and GitHub Copilot's skills, sub-agents, and standing
  instructions, because all three are formats it already understands. What it
  does not do is behave like those clients. Copilot's scoped
  `*.instructions.md` files declare an `applyTo` glob and are attached when
  Copilot is about to touch a matching file; Taurus assembles a brief once per
  turn, before it knows what the turn will touch, so it carries the glob into
  the prompt as a sentence and leaves the model to apply it. That is a weaker
  guarantee than Copilot's, and the file says so in as many words rather than
  quietly behaving as though it were the same. Frontmatter keys these tools have
  and Taurus does not — `handoffs`, `hooks`, `user-invocable` — are ignored
  rather than honoured, which is why a borrowed file is never rewritten in
  place. `.claude/rules`, which is Claude's spelling of the same scoped
  instructions, is the one directory in this family still unread. See
  [Instructions](capabilities.md#instructions).

- **The agent will not install an MCP server for you.** `draft_mcp_server` hands
  back an entry; adding it is yours to do, in the MCP panel or in the file. The
  command line is the whole of what a review could show, and it does not say
  what the program does, so this is a limit rather than a to-do. See
  [MCP servers](configuration.md#mcp-servers).
- **A PATH read from your login shell is a snapshot, not a subscription.**
  Taurus asks the shell once at startup, because a window launched from the Dock
  inherits the launcher's PATH and not yours. A server installed after that —
  `npm i -g` in a terminal beside the app — is invisible until Taurus is
  restarted, or until the entry names the program by its full path. The MCP
  panel says which directories it is searching rather than leaving that to be
  guessed at.
- **An agent's tools narrow what it is offered, not what it may do.** Every call
  a child makes goes through the same permission engine as the parent's, so
  `tools:` is a scope, not a sandbox. A per-agent permission policy would be a
  second thing to keep in step with the first, and is not there.
- **Stall detection needs an exact repeat.** Alternating between two dead ends
  is caught, but the calls have to match argument for argument. A model
  asking the same unanswerable question in three slightly different ways —
  reading a missing file by three spellings of its path — is making no more
  progress than one asking it identically, and nothing here notices. Judging
  that would mean deciding when two calls are *near* enough to be the same
  mistake, which is a guess the iteration ceiling makes unnecessary. See
  [When a turn stops](working-with-it.md#when-a-turn-stops).
- **The *model* still cannot produce an image.** A tool can hand one back, but
  the model itself reads pictures and cannot draw or edit one, so a turn
  best answered with a diagram answers in prose or reaches for `show_chart`.
  Closing that means image *generation*, which only some backends offer and none
  of them the same way.
- **No built-in tool returns an image yet.** The shape exists and every adapter
  carries it, but the only things exercising it are MCP servers — a browser
  driver's screenshot, a renderer's output. A built-in that rasterizes a PDF
  page or captures a window is a tool nobody has written here, not a limit of
  what a tool may return.
- **Only Anthropic carries a tool's image inside the result.** OpenAI's
  `role: "tool"`, Gemini's `functionResponse`, and Ollama's tool message are
  text, so on those three the picture is relocated to immediately after the
  result, with a marker line left where it was and a note naming the call. It
  arrives, in order, attributed — but it is a separate part of the conversation
  rather than part of the answer, and a model that weighs a tool result
  differently from a user message will weigh it differently. This is what the
  wire formats allow; closing it means those APIs changing, not this one.
- **A tool's image is budgeted at a flat estimate, like a pasted one.** 1,000
  tokens, regardless of its dimensions, because the real cost has nothing to do
  with the length of its base64 and each provider prices it differently. The
  number the compaction trigger reads is therefore approximate in exactly the
  place the stakes are highest — a turn that returned four screenshots may have
  less room left than the counter says.
- **Trimming an old tool result drops its picture.** Deliberate: the point of
  shortening an old result is to reclaim the window, and the image is the most
  expensive thing in it. But it means a screenshot from earlier in a long
  conversation is gone from the model's view while its caption remains, and
  nothing says which it was.
- **An attached image is not in the checkpoint log.** It goes into the
  transcript, so it survives and redraws; it is not a file in the workspace, so
  a rewind neither restores nor reports it. That is correct — there is nothing
  to put back — but it means a conversation's disk footprint grows in a place
  the **Changes** drawer does not account for.
- **The first index is slow, and a search that arrives early still waits for
  it.** Embedding this repository takes around 44 seconds. Sending a message
  starts that in the background, so most of it is usually done before anything
  searches — but a model that reaches for `search_code` in its first tool call
  waits for the rest of it inside that call. What is left is genuinely less:
  the search takes the warm-up over rather than starting again, everything
  embedded so far is already written down, and the turn watches a passage count
  move. Closing it the rest of the way means answering from a partial index and
  saying so, which is a different promise from the one the tool makes —
  every search refreshes first, so that a file just written is a file that can
  be found.
- **The index does not notice a file that changed without moving.** Length and
  modification time, the same comparison the sweep uses and blind in the same
  place: a rewrite to the same length within one filesystem tick is invisible,
  and the stale chunk stays until something else about the file moves.
- **Semantic search is only as good as what ranks it.** `search_code` ranks by
  cosine similarity, and optionally by a reranking model over the top thirty of
  those — but there is still no keyword fallback and no blend with grep. A query
  that lands badly returns three confident near-misses, and the tool says they
  are leads rather than answers, which is the whole of what it can do about it.
  Where the literal text is known, grep is exact and this is only close.
- **Nothing here embeds in-process.** An index needs a backend with an
  embedding endpoint, and every provider this speaks to except Anthropic has
  one — so this is a gap for exactly one setup: chatting to Claude with no
  other backend reachable. Closing it means running an embedding model inside
  this process, which is a model to download on first use and a machine-learning
  runtime to carry on all three platforms, for a case a second provider entry
  already answers. Not built, and not obviously worth building.
- **Reranking needs a second server, and most backends cannot be it.** The
  `/rerank` route is Cohere's shape rather than OpenAI's, and OpenAI never
  shipped one to imitate — so it is served by llama.cpp started with
  `--reranking`, by text-embeddings-inference, and by the hosted rerankers, and
  by almost nothing else. Ollama, which is where most local setups embed, has no
  such route at all, which is why `rerank_provider` exists as a setting separate
  from the embedding provider. Closing this properly means running a reranking
  model in-process rather than asking for an endpoint, which is the same
  unbuilt thing that would let the index work with no local server at all.
- **A reranked score cannot be compared to anything but its own result set.**
  Voyage and Cohere normalize to 0–1; llama.cpp returns the cross-encoder's raw
  logit, where negative values are ordinary. Taurus orders by it and never
  filters on it, and labels the column `relevance` rather than `similarity` so
  the number is not read as a cosine — but there is no way to make one backend's
  0.82 mean the same thing as another's, and there is no threshold below which a
  result is known to be worthless.
- **The Traces panel covers one run of the app, and nothing else.** The spans
  it draws live in a ring in that process: quitting forgets them, a `taurus
  run` in the terminal is a different process and never appears in the window,
  and once the ring is full the oldest go. It says how many it has forgotten
  rather than describing a shorter period than it appears to, but "everything
  since launch" is the widest question it can answer. Anything longer than a
  session is what an OTLP endpoint is for, and the two are not alternatives —
  the same spans go to both.
- **Traces go out over HTTP, and only over HTTP.** OTLP has a gRPC transport
  too and this speaks the `http/protobuf` one alone. Every collector worth
  naming accepts it, so this is a smaller gap than it sounds — but a setup
  standardized on gRPC needs a collector in front, and the port is the other
  one (4318 rather than 4317), which is the mistake everybody makes once.
- **A trace says which tools ran, not what they were called with.** Arguments
  are absent from tool spans even when content capture is on: they are the one
  place a path, a URL, or a command line would end up on a dashboard with no
  way to notice. Closing it means deciding what an argument may contain, which
  is the same unanswerable question redaction always is.
- **Cache and reasoning tokens are only as good as the backend's report.**
  Anthropic reports cache reads and writes, OpenAI-compatible servers report
  cached prompt tokens and reasoning tokens when they have them, Gemini reports
  cached content and thoughts. Ollama reports none of it, and a compatible
  gateway may report a subset or nothing. Absent is recorded as absent rather
  than zero — but that means a dashboard comparing two backends is comparing
  what each chose to say.
- **Reasoning tokens are inside the output count, not beside it.** Every
  backend that reports both counts reasoning within `output_tokens`, so adding
  the two double-counts. The field is kept because it is the only way to see
  that a turn spent its budget thinking rather than answering.
- **`fetch_url` reads the HTML it is served.** No JavaScript runs, so a page
  that renders its content client-side comes back near-empty. Closing this
  means shipping a browser engine, so it is a limit rather than a to-do.
- **`fetch_url`'s address check does not survive a proxy.** Loopback and
  private-network addresses are refused, and the check runs inside the
  client that connects, so a name cannot answer publicly for the check and
  privately for the connection. An HTTP proxy resolves the name at its end
  though, so a request routed through one reaches a destination Taurus never
  sees. Taurus configures no proxy, but reqwest reads `HTTP_PROXY` and the
  system settings, and refusing to work behind a corporate proxy would cost
  more than this buys. `"allow_private_hosts": true` in `search.json` turns
  the check off deliberately.
- **Config is re-read at turn boundaries, and nothing is watched.** Instructions,
  sub-agents, skills, and hooks are all fingerprinted — a `stat` of the files
  behind each — and re-read at the start of a turn when that fingerprint moved.
  So an edit lands on your next message rather than the next launch, and coming
  back to the window runs the same check on top of that. What
  is deliberately absent is a file watcher: one fires whenever an editor happens
  to save, which is routinely the middle of a running turn, and the brief a turn
  was given and the roster it delegates against have to be the ones it started
  with. The costs are a one-turn delay in the worst case, and the same
  same-length-same-tick blind spot every fingerprint here has. Two things are
  *not* on this path and still need a reload: the provider list and
  `settings.json`. Both are edited in the app rather than in a file most of the
  time, and both are re-read when they are saved there — a hand edit to
  `providers.json` while the app is open is the case that still waits.

- **The terminal dock is a terminal, not part of the conversation.** It runs
  your shell in the window the agent works in, and that is the whole of the
  connection between them. The agent cannot read what you ran there, you cannot
  hand it a failed command without copying the text across, and its own
  `run_command` calls appear in the transcript rather than in the pane. All
  three are the same missing piece: the shell has no way to say where one
  command ended and the next began, so there is nothing for either side to point
  at. Closing it means shell integration — the `OSC 133` marks a prompt emits
  around each command, injected per shell — which is what would turn a
  scrollback into addressable blocks with an exit code and a duration on each.
  That is the next thing to build here rather than a limit. See
  [Terminal](capabilities.md#terminal).
- **What you run in the terminal is outside the undo history.** Every command
  the *agent* runs is bracketed by a sweep of the workspace, so anything it
  changed can be put back by a rewind. A command you type in the dock is not:
  the shell runs it directly, nothing reads the workspace before or after, and a
  `sed -i` there is invisible to the Changes drawer and to every checkpoint. The
  dock does not pretend otherwise — it is a terminal, and a terminal has never
  had an undo — but it is worth knowing that the two halves of the window keep
  different promises. Covering it needs the same command boundaries the entry
  above is about, and it would cost a read of the workspace per command you
  type. See [Rewinding a turn](safety.md#rewinding-a-turn).
- **One shell, and it ends when the dock does.** There are no tabs and no
  splits, and hiding the pane is not hiding it — closing the dock ends the
  shell, the same as closing a terminal window. So a long `cargo build` started
  there does not survive ⌃`, and there is no second pane to run something else
  in while it works. Both are worth having and neither is written; a shell that
  outlived the pane would also need somewhere for its output to go while nothing
  is watching, which is a scrollback the backend would have to keep.
- **A prompt's icons need a font this app cannot ship.** Powerline separators
  and the git glyphs a modern prompt draws come from the private-use area, which
  the app's own mono has nothing in. The dock names the Nerd Fonts people
  actually install and falls back through them, so a machine with any of them
  renders the prompt correctly — and a machine with none shows those glyphs as
  empty boxes, with the text around them intact. Bundling one would be tens of
  megabytes for a decoration, and there is no setting to name a different font
  yet.
- **On Windows the dock holds a console window open for as long as it is open.**
  The ConPTY gap above is the same bug seen for longer: a release build has no
  console of its own, so a pty opened without the sideloaded runtime creates
  one, and where a tool call showed it for the length of a command the dock
  shows it for the length of the session. The fix is the same — the two files
  the Windows bundle ships beside the executable — and the startup log line
  saying whether they were found is still the only warning available.
- **A query answers thirty rows, and that is a context limit rather than a
  reading one.** `query_data` results are read by the model, and every row is
  paid for again on each later request of the turn — so the tool is shaped for
  aggregating, and a query that wants a thousand rows wants to be writing a
  file. The pane pays the same cap even though nothing there is paying for
  context, which is the honest cost of one guarantee rather than two: the pane
  and the model go through the same call, and the alternative is a second limit
  that can be got wrong on its own. A result that hit the cap says so.
- **A refused query is refused by plan shape, not by intent.** `query_data`
  plans every statement and rejects it if the plan does anything but read, and
  the match over plan kinds is written out in full so that a future DataFusion
  release adding a writing statement fails to compile rather than being waved
  through. What that does not cover is a read that is merely *expensive*: a
  cross join over two million-row files is a legal SELECT. There is a 512 MB
  ceiling per query so one of those fails instead of taking the app with it,
  and the tool is cancellable, but nothing estimates a query before running it.
- **A recipe transforms; nothing here judges.** A recipe is a chain of SQL
  steps, so it can clean, filter, join, derive, and rank — everything SQL can
  express. What it cannot do is anything that needs a *model*: classify a free
  text column into a taxonomy, extract fields from a description, embed a
  column for a recommender. Those are the reason the whole feature exists for
  anybody building a dataset for an agent, and none of them is written. They
  need a different shape from a SQL step, because a judgement over a million
  rows is a bill: it wants to be sampled first, reviewed, and only then
  committed to the whole file. Adding one as another `-- step:` would skip
  exactly the gate that makes it safe to run.
  See [Recipes](working-with-it.md#recipes).
- **What the Data pane was showing reaches the model but not the transcript.**
  A message sent from the pane carries the open dataset and the query box, so
  "this" has a referent — but it goes onto the prompt, not onto the transcript's
  copy of what was said. That is the same split a `/command` expansion makes,
  and it has the same consequence: a conversation reopened a week later shows
  "which category refunds most?" with no record of which dataset that meant.
  The chip above the composer is what makes it visible at the time, and the
  answer beneath usually names the dataset, which is what makes this bearable
  rather than fine. Fixing it properly means a transcript entry that can carry
  more than text and images.
- **The turn strip says what is happening, not what was said.** One line above
  the composer while a turn runs in the Data pane, showing the running tool or
  the last sentence of prose. It is not a transcript and cannot be: a table or
  a chart has nowhere to go on one line. The question card — which was the case
  that mattered, because the turn parks and only you can unpark it — is
  called out rather than left to be inferred: the strip switches to a breathing
  mint ring and says *Waiting on your answer*. What is still true is that the
  answer itself has to be given in the conversation, and the strip cannot show
  you the options.
- **The waveform's shape is the tool's category, which is coarser than the
  work.** Four shapes over six categories, and the categories are themselves a
  simplification — `grep` and `read_file` are both reads and draw the same
  sweep, though one is a search and the other is not. Finer would mean the
  harness classifying tools by something other than effect, which is what the
  categories exist to do and what the run header counts. The shape is a useful
  hint about the kind of work, not a readout.
- **Nothing says how long a turn has been running.** The motion says a turn is
  alive; it says nothing about whether *alive* has meant forty seconds or four
  minutes. The design's own working state pairs its waveform with an elapsed
  counter, which Taurus cannot draw honestly — a tool call carries its own
  start time, but a turn does not, and a resumed conversation carries neither.
  A finished run reports its duration in the run header, which leaves exactly
  the case you would want it in uncovered.
- **A query card stands alone, so a query-heavy turn is a stack of cards.** Any
  tool call that draws a view is excluded from the folded run header — that is
  what stops a table being filed under "6 steps · 11s" behind a disclosure
  triangle. `query_data` draws one, so a turn that asks six questions leaves
  six cards rather than one row of six. Each is small and
  each is useful; six in a column is still more transcript than the turn is
  worth. The fix is a fold that can hold cards, which is a change to how a run
  is drawn rather than to this tool.
- **Running a query from a card can disagree with the transcript above it.**
  The card carries the SQL and not the rows, on purpose — a remembered answer
  is the number that is right for a week and then quietly wrong. So **Run in
  Query** asks the files as they are *now*, and if something has rewritten one
  since, the pane's answer and the model's sentence about it differ with
  nothing saying why. That is the right way round — the fresh number is the
  true one — but the disagreement is left for the reader to notice.
- **A recipe's steps cannot be taken to the query box.** The pane shows every
  step's SQL when a recipe is opened, and there is deliberately no button to
  run one: every step reads from `input`, which is the rows the step before it
  produced, and no such table exists outside a run. Pasting one into the box
  gets `table 'input' not found`, which would be the button's fault rather than
  the user's. Making it work means materializing the chain up to that step,
  which is most of a run — see the sample-then-commit gating that phase 3 needs
  anyway.
- **The drafts these buttons write are a guess at the question.** "Add this as
  a step in a recipe" does not say *which* recipe, because the pane does not
  know — so the model asks, or picks, and either way it is a round trip the
  person could have saved by typing four words. The button is a head start, not
  a complete instruction, which is why nothing is sent and the cursor is left at
  the end of it.
- **The query box's highlighting is a scanner, not a grammar.** It knows where
  a literal starts and ends, which is the half a regex gets wrong, and it knows
  nothing about scope. The function list is a fixed set of the common ones, so
  a DataFusion function nobody thought of — and any UDF — draws as a plain
  identifier rather than as a call. Nothing is *wrong* on screen when that
  happens; a word is simply the wrong colour, which is the failure mode a
  scanner is chosen for.
- **Completion knows the files, not the query.** It offers columns that exist
  in a loaded dataset, and a CTE's output columns do not — `WITH t AS (SELECT
  a + b AS total …) SELECT | FROM t` will not offer `total`, because knowing it
  exists means planning the query, which is the engine's job and a round trip
  away. Aliases are found by sweeping for `FROM`/`JOIN` and the name after it,
  so the subquery form `FROM (SELECT …) t` is not matched either and `t.` falls
  back to offering every column in the workspace. Both cases degrade to a
  longer list rather than to a wrong one.
- **The caret the completion list hangs off is computed, not measured.** The
  box is monospace and does not wrap, so the list's position is arithmetic: one
  cell width, times the column, plus the padding. A character that is not one
  cell wide — CJK, most emoji — puts the list a few characters off for the rest
  of that line. The alternative is measuring a mirror element on every
  keystroke, which is a lot of DOM for a case that does not arise in SQL.
- **A selection in the query box shows as a block of colour with no text in
  it.** The consequence of painting the query on a layer behind a transparent
  textarea: the browser draws the selection on the real control, whose text is
  invisible. The highlight is tinted harder than the app's default to
  compensate. Fixing it properly means an editor component, which is the
  dependency the whole arrangement exists to avoid.
- **The tables panel is reference and nothing is clickable in it.** Deliberate
  rather than unfinished — completion is the way text gets into the box, and a
  second insertion route is a second set of rules about where the caret lands.
  It does mean a column read there still has to be typed, and the first three
  letters are all that costs.
- **Identifier case is Taurus's own dialect choice, and a recipe carries it.**
  DataFusion lowercases an unquoted identifier by default, the way Postgres
  does; Taurus turns that off, so a column reported as `Material` is written as
  `Material`. That is the right trade for data whose column names come from a
  spreadsheet header — it makes the tool's own output valid input to itself —
  but it does mean a recipe written here is *stricter* than the same SQL pasted
  into a database client, where `SELECT MATERIAL` against a lowercase column
  would have worked. Nothing warns about that when a recipe is copied out.
- **A recipe's SQL is DataFusion's SQL, and that goes into your
  repository.** The engine sits behind a trait so the rest of the harness does
  not name it, but a recipe is a file with SQL text in it, so swapping engines
  would mean every recipe anybody wrote is a file in a dialect nothing reads.
  That cost was taken knowingly — the alternative is an invented step language,
  which buys portability nobody wants with unfamiliarity everybody pays — and
  it is the reason exactly one method on the trait writes. What is not written
  is any way to *tell* you a recipe uses something dialect-specific; a
  `row_number() OVER` is portable and a DataFusion-only function is not, and
  nothing distinguishes them.
- **A recipe writes one file, and there is no incremental run.** Every run
  reads the source from the beginning and rewrites the output whole. There is
  no "only the rows since last time", no partitioning, and no way to append —
  so a recipe over a growing export costs the whole export every time. Each
  intermediate step also spills to a scratch file, which keeps memory flat and
  the row counts exact but means a five-step recipe over a gigabyte does
  several gigabytes of temporary I/O. That scratch goes in the system temp
  directory, so a machine with a small `/tmp` is the case that fails; there is
  no setting to move it.
- **Running a recipe from the pane asks nothing first.** The button carries the
  path it writes, and that is the whole of the consent — the same arrangement
  the query box has. `run_recipe` called by the *model* does prompt, and its
  prompt names the path parsed from the same file the run will use. What
  neither offers is a preview of the output before it lands: there is no dry
  run, no "this would drop 380,000 rows, continue", and the per-step deltas
  arrive after the file is already written. A rewind undoes it, which is what
  makes that acceptable rather than fine.
- **Writing or editing a recipe is not rewindable.** `.taurus` is skipped by
  the checkpoint sweep, so a turn that authors a recipe leaves nothing for
  `taurus rewind` to put back — the same property project skills already have,
  for the same reason: these are the instructions, not the output. The file a
  recipe *writes* is fully rewindable. Git is the undo for the recipe itself,
  which is an argument for committing them and not much comfort before the
  first commit.
- **The list of loaded datasets does not travel with the repository.** It lives
  in `~/.taurus/data/<workspace>/datasets.json`, beside the transcripts and the
  search index, not in the project's own `.taurus`. That is what keeps loading a
  file from being a *write* to the workspace — otherwise looking at a CSV would
  cost a permission dialog, a diff in the Changes drawer, and a line in the next
  commit. The cost is that a teammate who clones the repository loads the files
  again, which is one sentence to the agent. A recipe sidesteps it by naming its
  own files — `source: data/events.csv`, plus a `tables:` block for anything it
  joins against — which is what makes a committed recipe run on a fresh clone.
  A recipe that names loaded datasets instead still does not, and nothing warns
  you which kind you have written.
- **A profile is a full scan every time, and it cannot be cancelled.** Nothing
  is cached: a dataset entry points at a file anything can rewrite, and a
  remembered row count is the kind of number that is right for a week and then
  quietly wrong. So opening the pane on a multi-gigabyte file reads it again,
  and clicking away leaves that read running to completion rather than stopping
  it. Caching it properly means invalidating on the file's length and
  modification time — the same rule the search index already uses — and
  cancelling means threading a token through the engine trait. Neither is
  written. What keeps this bearable today is that the scan is the *only*
  expensive operation: loading reads a header, and paging is flat in the offset.
- **`.json` means newline-delimited JSON, not a JSON array.** A file holding one
  big `[ {...}, {...} ]` is refused with a message rather than read, because
  reading it would mean parsing the whole thing into memory before any of the
  streaming below it could start — which is the one shape of file this is
  supposed to protect you from. Converting it is one `jq` line and the agent can
  run it. There is no Excel reader either, and adding one is a dependency rather
  than a design question.
- **A nested column is counted and not described.** A list, a struct, or a map
  profiles as how many rows have one and nothing else — no distinct count, no
  range, no common values, because none of those are questions with an answer
  until the column is flattened. It is kept rather than refused so that one
  nested column in an export does not cost you the other thirteen. Flattening is
  a transformation: a recipe can flatten one with an `unnest` step, and until
  somebody does, the profile says what it can.
- **The grid does not sort or filter.** It pages, a hundred rows at a time, in
  the order the file is in. Sorting a million rows is a query rather than a
  click — it has to go back to the engine, and the pane would need somewhere to
  say that it is running one — and filtering is the same thing with a predicate.
  Both are worth having. The transcript's `show_table` sorts because its rows
  are already in the browser; these are not, and pretending otherwise would sort
  the hundred rows on screen and call it sorted.
- **Searching a conversation reads every transcript, every time.** There is no
  index. What makes that affordable is the shape of the file rather than any
  structure kept beside it: a transcript is JSONL, so one whose bytes do not
  hold the query cannot hold it once parsed, and a conversation that does not
  match costs one read and nothing else. Measured on sixty-one real
  conversations across every workspace, a whole-history search is about 110ms —
  which is why the palette debounces rather than searching per keystroke, and
  why its two local groups answer first and this one fills in underneath. It
  grows linearly with how much you have said. Building an index means deciding
  when to rebuild it, and a stale index that quietly stops finding last
  Tuesday is worse than a search that takes a tenth of a second. See
  [Finding a conversation](working-with-it.md#finding-a-conversation).
- **The search is literal, and it does not read tool calls.** No regex, no
  fuzzy matching, no stemming: `banner` does not find `banners`. And it reads
  prose only — what you typed and what the model wrote back, not a tool's
  arguments and not its results. That last one is a decision rather than an
  omission, and it is what makes the results usable: tool results are file
  contents and build logs, so including them would match nearly every
  conversation for nearly every query. The cost is real, though — a thing that
  only ever appeared in a file the agent read is not findable here, and `grep`
  over `~/.taurus/sessions` is the honest answer for that.
- **A search hit is found again by text, not by position.** The search reports
  which message matched, and the app throws that away and looks for the words
  again in what is on screen. The two do not count the same things — a turn
  folds a prompt, an answer and a run of tool calls into one card — and looking
  again is both simpler and right for a conversation that has been compacted
  since. What it costs is the case where the hit was summarized away: the
  conversation opens, nothing is marked, and nothing says why. It also marks
  the *first* turn holding the words rather than the one the search found, which
  differ when a conversation says the same thing twice.
- **Colouring code is a scanner, not a parser.** One walk serves every
  language, parameterized by how a comment opens, which delimiters quote a
  string, and which words are the vocabulary. That is enough to be right about
  ordinary code and it is not enough to be right about all of it: a construct
  it misreads is coloured wrongly rather than reported, because there is
  nothing here that could report it. The languages it knows are Rust,
  TypeScript and JavaScript, Python, Go, shell, SQL, JSON, YAML, and TOML.
  Everything else — HTML and CSS included, which are common in a fenced block
  and whose syntax is not word-shaped — renders plain with its label intact.
  Growing the list is a `Grammar` each; growing it to *markup* is a second
  scanner, and a word-oriented one turned loose on HTML produces confident
  nonsense.
- **An intra-line diff mark is a trim, not a diff.** The common words at each
  end of a replaced line come off and whatever is left in the middle is marked,
  which is one region per line by construction. A line with two separate small
  edits in it is therefore marked from the first to the last, including the
  unchanged text between them. Two cases decline outright rather than guess: a
  line rewritten end to end, where marking almost all of it would look like a
  finding, and a run of removals answered by a run of additions of a different
  length, where pairing by position would mark the difference between unrelated
  lines. In all three the line-level `+` and `−` are still exactly right, which
  is why declining is affordable.
- **What a tool cost in the Context panel is apportioned, not measured.** The
  provider reports one number for a whole request and never says which part of
  the prompt was whose, so every figure there except the billed row is the
  harness's own four-characters-a-token estimate — the same estimate the
  compaction threshold runs on, and limited the same way. It is accurate enough
  to rank tools against each other, which is what the panel is for, and it is
  not a bill. The two numbers that are exact are what the provider reported in
  and out. See [The context window](working-with-it.md#the-context-window).
- **The panel accounts for tokens, not money.** No provider's prices are in
  here and none are fetched, so nothing multiplies the billed tokens by a rate.
  Adding it means a price table per provider per model that somebody has to
  keep current, and a table that is six months stale reporting dollars to two
  decimal places is worse than no dollars at all.
- **There are four keyboard shortcuts.** ⌘K and ⌘⇧P open the palette, ⌘N starts
  a conversation, ⌘, opens Settings, and ⌃` shows the terminal. That is the
  whole list, and it is short on purpose: every one of them is also a row in
  the palette wearing the key it answers to, so the palette is the discovery
  surface and adding a fifth shortcut is cheap in a way adding the first was
  not. What is not here is user-defined bindings — a keymap file means a
  conflict resolver, a way to see what is bound, and a way to find out why a
  key did nothing.
- **Chunking for the index is a line window, and structure-aware chunking was
  tried and lost.** Forty lines with ten of overlap, in every language. The
  obvious improvement is to cut where a definition starts, and the obvious
  objection — a grammar per language, a silent fallback for the ones you lack,
  and confident nonsense on a file half understood — turns out not to apply to
  reading *layout*: a non-blank line at zero indent, after a blank line or after
  the close of what came before, starts a new top-level thing in every language
  a person writes by hand, and snapping a cut to the nearest one within twelve
  lines needs no grammar and has no second code path.

  It was built that way, and measured. On this repository, fifteen questions,
  `nomic-embed-text`: line windows scored MRR 0.668 and put the answering file
  first 53% of the time; structure-snapped cuts scored 0.598 and 40%; adding an
  embedded heading — the file's path and the definitions the chunk sits inside —
  scored 0.565 and 40%. Restoring the overlap that snapping drops recovered
  nothing (0.577), which rules out the obvious confound. The numbers are
  deterministic and reproduced exactly across runs.

  So it is not shipped, and `git show` on the commit before the one that
  reverted it is the implementation if anybody wants to try again. What the
  measurement does **not** settle: fifteen questions is a small sample, one
  embedding model is one embedding model, and the corpus is Rust and TypeScript
  — a model that reads code structurally, or a workspace in a language where
  indentation carries more, could land differently. One thing it did not isolate
  is that snapping produces 13% fewer passages (4025 against 4610), and a corpus
  with more passages in it gives every file more chances to be the best match
  for something; the overlap control lengthened chunks rather than adding them,
  so that axis is untested. `cargo run -p taurus-index --example retrieval` is
  the gate, and re-running it is the whole cost of arguing with any of this —
  though it has to be run the way that comparison was, scoring both things
  against one corpus in one process. The corpus is the working tree, and
  editing a doc page between two runs was measured moving MRR by 0.03, which is
  the size of the differences it exists to detect.
- **Reranking is still off by default, and still ungated.** `rerank_model` is
  empty because the plan that added it said to beat cosine before defaulting it
  on, and that comparison has never been run. It has somewhere to be run:
  the retrieval harness above scores whatever the index currently does, so the
  gate is one command with the setting on and one with it off. Until somebody
  does that, an empty default is the honest state rather than a forgotten one.
- **A theme sets fourteen colours, three typefaces, a wordmark and a corner
  radius, and nothing else.** Not a stub — the ceiling is the point. Everything
  below the top of `src/styles.css` speaks in roles, so those fourteen values
  move the whole window; letting a theme restate a *rule* instead would let it
  break a layout in a way only its author could reproduce, and "the app is
  broken" would be a report nobody could tie back to a colour picker. The
  spacing ladder is deliberately not exposed for the same reason: it is a
  constraint the stylesheet's own tests enforce, and a theme that could redefine
  it could make the app look like nobody measured anything. What that costs is
  real — a theme cannot change a font size, a weight, a shadow, or the width of
  the rail.
- **A theme cannot bring a typeface with it.** `fonts` names families, and they
  have to be installed on the machine already: the window's CSP allows no remote
  stylesheet, so there is nothing to point a `@font-face` at. Bundling a font
  file inside a theme would mean reading arbitrary binaries out of a config
  directory and injecting them as `data:` URIs, which is a wider hole than the
  feature is worth. A theme naming a font nobody has degrades to the stack the
  app ships rather than failing, so this is quiet rather than broken — and quiet
  is its own problem: nothing on screen says the font was not found.
- **Themes are read fresh on every status, and only the active one carries its
  logo.** The picker's full scan happens when the picker opens. That split
  exists because a resolved theme carries its logo inlined as base64, and the
  status is pushed after anything that moves a number on screen — so a scan on
  that path would re-encode every logo on the machine several times a turn. The
  cost is that a *different* theme's problems, in a file you are not using, are
  not reported until you open Settings › Appearance.
- **A hook's timeout kills the hook, not its grandchildren.** `timeout_seconds`
  is enforced by killing the program the hook names — the whole of it, including
  the wait for a hook that never reads the payload off its stdin. What it does
  not reach is anything that program started and left running: a hook whose
  script backgrounds a job and returns has put that job outside what a timeout
  can undo. Covering it means a process group per hook — `setsid` and `killpg`
  on Unix, a Job Object on Windows — which is the same gap every other child
  process in the harness has, since `kill_on_drop` is what the terminal and the
  skill scripts use too. What it costs today: a hook that leaks a background
  process leaks one per call, and nothing in the app will say so. The direct
  case, which is the one people write, is covered and tested.
