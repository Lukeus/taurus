# Known gaps

<sub>[← Taurus AI Shell](../README.md)</sub>

What Taurus does not do, stated where it can be read before it is discovered.
Each entry says what is missing, and what covering it would cost.

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
  command moved them — a tree that matches neither commit. The turn now says so
  when it happens, and points at `git reflog`, but covering it properly means
  snapshotting the object store, which is its own feature. The warning also
  reaches you at the moment the command runs rather than at the moment you
  reach for undo; carrying it into the checkpoint log means a record shape and
  a format version, and has not been done. Staging is unreported as well — see
  [Rewinding a turn](safety.md#rewinding-a-turn) for why the index is deliberately not
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
- **Committing a turn does not know about the turns around it.** Each commit is
  offered on its own, so committing turn 3 and then turn 5 leaves turn 4's work
  in the tree, uncommitted and now sitting on top of a commit that does not
  include it. Nothing warns about that ordering, and nothing offers to squash a
  run of turns into one commit. Both want a model of which turns are already in
  `HEAD`, which is a record the checkpoint log does not keep.
- **A committed turn can still be rewound.** The two features do not know about
  each other: rewinding past a turn you committed restores the files and leaves
  the commit in place, so the tree no longer matches it. `git` has the way back
  and the drawer does not say so at that moment. Wiring them together means the
  checkpoint log recording commits, which is a record shape and a format version.
- **Nothing makes a model plan.** `update_plan` is offered and the prompt says
  when to reach for it, and that is the end of the harness's leverage. On a
  five-step mechanical task neither `qwen3.6:27b` nor `qwen3.5:9b` called it
  unprompted — both simply did the work — so the feature earns its keep on the
  long, exploratory turns where drift actually happens, and on the models that
  take the instruction. Forcing a plan on every multi-step request would spend
  an iteration and a card on turns that never needed one.
- **A plan can end in progress.** Nothing requires the last `update_plan` of a
  turn to mark the final step done, and a model that finishes the work and goes
  straight to its answer leaves a step reading `[>]` that is actually complete.
  The turn is over so nothing reads it back, but the pinned panel keeps the
  stale version — and being pinned, it keeps it somewhere you cannot miss.
  Closing it means the harness deciding a plan is finished, which it cannot
  know; the panel clears on the next request instead.
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
- **A branch is recorded, not enforced.** A conversation started on `feat/x` and
  resumed on `main` is labelled in the rail and nothing more — its rewind will
  still restore pre-images from a tree that is no longer checked out, and its
  file references still point where they pointed. Refusing or warning at the
  moment of a rewind would be the useful version; it needs the branch carried
  into the checkpoint log rather than only the transcript header.
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
  [Sub-agents](capabilities.md#sub-agents).
- **Taurus will not install an MCP server for you.** `draft_mcp_server` writes
  a block to paste; adding it is yours to do. The command line is the whole of
  what a review could show, and it does not say what the program does, so this
  is a limit rather than a to-do. See [MCP servers](configuration.md#mcp-servers).
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
- **An image can only be sent, never received.** The model reads pictures and
  cannot produce or edit one, so a turn that would be best answered with a
  diagram answers in prose. That is a limit of what the normalized types carry
  in the other direction, and closing it means a shape for image output that
  only some backends could fill.
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
- **Semantic search is only as good as the embedding model.** `search_code`
  ranks by cosine similarity and nothing else — no reranking, no keyword
  fallback, no blend with grep. A query that lands badly returns three confident
  near-misses, and the tool says they are leads rather than answers, which is
  the whole of what it can do about it. Where the literal text is known, grep is
  exact and this is only close.
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
