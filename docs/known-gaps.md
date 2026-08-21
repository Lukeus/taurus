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
  list, so the warning now arrives at the moment you reach for undo rather than
  only at the moment the command ran. Staging is still unreported — see
  [Rewinding a turn](safety.md#rewinding-a-turn) for why the index is deliberately not
  watched.
- **A change that moves neither length nor timestamp is invisible.** The same
  walk compares size and modification time, which is what `make` and `rsync`
  have always compared. On a filesystem with nanosecond timestamps defeating it
  takes deliberate effort; on one with coarse timestamps, a command that
  rewrites a file to the same length within the same tick would slip through.
  Closing it means reading every file twice per command.

  The commands after the first in a turn now reuse what the previous one read,
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
- **A diff is shown for `write_file` and `edit_file` and nothing else.** A
  command line has no before-and-after to compute, which is exactly why
  `run_command` is swept afterwards rather than predicted. So the most
  consequential writes in a session — the ones a script made — are still
  approved on the command line alone. They are at least *readable* afterwards
  now: the **Changes** drawer diffs what the sweep recorded, so a `sed -i` across
  a dozen files can be inspected line by line once it has happened. That is
  review after the fact, not before it.
- **A turn's diff attributes a hand edit to the wrong turn.** What a turn
  changed is computed as its own pre-image against the next recorded pre-image
  of the same file, which is exact for anything Taurus did and silently wrong
  for anything you did in between — your edit appears inside the later turn's
  diff. Closing it means post-images, which is a second copy of every file
  written per turn to attribute a case the rewind already warns about in the
  same words. See [Keeping a turn](safety.md#keeping-a-turn).
- **Committing a turn still does not offer to squash.** The checkpoint log now
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
  has changed is that the rewind plan now names the commit and what to do about
  it (`git revert`, `git reset`) before you press anything, rather than letting
  you find out afterwards. The one thing it will not do is refuse: a rewind that
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
  deliberate is having nowhere to read it afterwards, and that is now fixed —
  every delegate keeps its own transcript beside its parent's, written as it
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
  is not visible until the next one. That is now the whole of it: the
  directories are checked at every turn boundary and rescanned when anything in
  them moved, so a new agent is available on the next message rather than after
  a reload or a trip to the drawer. The remaining freeze is the feature — a turn
  must delegate against the roster it started with — and it is why there is no
  file watcher here rather than an admission that one is missing. The
  same-length-same-tick blind spot above applies to the check here too. One
  surface still lags on purpose: the `/` command *menu* lists the last scan,
  because it redraws on every keystroke and taking config locks there is how a
  reload deadlocks against typing — typing the name in full works immediately,
  and the menu catches up after the next message. See
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
  is now caught, but the calls have to match argument for argument. A model
  asking the same unanswerable question in three slightly different ways —
  reading a missing file by three spellings of its path — is making no more
  progress than one asking it identically, and nothing here notices. Judging
  that would mean deciding when two calls are *near* enough to be the same
  mistake, which is a guess the iteration ceiling makes unnecessary. See
  [When a turn stops](working-with-it.md#when-a-turn-stops).
- **The *model* still cannot produce an image.** A tool can hand one back now,
  but the model itself reads pictures and cannot draw or edit one, so a turn
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
- **The first index is slow, and inside a turn it still blocks it.** Embedding
  this repository takes around 44 seconds. **Settings → Search → Build index
  now** pays that where you can watch it move and stop it, which is the way to
  avoid the problem rather than a fix for it: a workspace whose index is built
  on the first `search_code` still spends a tool call on the whole of it. The
  turn reports its way through now — a passage count every few per cent, rather
  than one line and forty-four seconds of silence — but the model still sees
  only a call that has not returned.
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
