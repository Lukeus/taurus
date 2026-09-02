# Development

<sub>[← Taurus AI Shell](../README.md)</sub>

```bash
cargo test --workspace     # 1466 tests
pnpm test                  # transcript reducer, replay, settings, rewind, diffs
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# What a token costs to draw. See "Drawing a token", below.
pnpm bench

# The README's screenshots. Needs Chrome or Chromium; nothing else does.
pnpm screenshots

# TypeScript types are generated from Rust; regenerate after changing a payload.
# `src/bindings` is not committed, so this is also the first thing a fresh clone
# needs — without it `pnpm build` cannot find a single frontend type.
pnpm bindings
```

## Drawing a token

```bash
pnpm bench     # src/components/Transcript.bench.tsx
```

The number to read is not any one row; it is whether the rows climb.

A transcript grows all day. A renderer that redraws the whole conversation for
every token gets slower the longer the conversation runs, which is the shape of
slowness people report as "it was fine this morning" — and it is what this used
to do. The four cases differ only in how much conversation sits *above* the turn
being streamed into, so a healthy result is four numbers that stay flat:

```
· under  1 turn  of history    0.111 ms
· under  5 turns of history    0.093 ms
· under 20 turns of history    0.121 ms
· under 50 turns of history    0.159 ms
```

Before the transcript was memoized per turn the same four read 0.173, 0.395,
1.449, and 3.406 — a twenty-fold climb, and 22x the cost at fifty turns.

Two things hold that flat, and both are load-bearing:

- **Turns carry their identity forward.** `reuse` in `Transcript.tsx` hands back
  the object it built last time for any turn whose entries are the same objects,
  which is what gives the per-turn memo something it can compare. The reducer
  never rewrites an entry it did not touch, so this is exact rather than a
  heuristic. `Transcript.test.tsx` covers it, because a refactor that dropped it
  would cost nothing visible and quietly restore the old behaviour.
- **The transcript holds its callbacks steady itself.** A caller writing
  `onAnswer={() => …}` inline — the natural way to write it, and what
  `DelegateTranscript` does — would otherwise hand every turn a new function on
  every token and undo all of the above. That is a property worth owning rather
  than a rule callers have to know.

Underneath both, the store batches a frame of stream events at a time rather
than writing once per token (`batchEvents`), so a fast local model produces
thirty-odd renders a second instead of hundreds.

jsdom lays nothing out and paints nothing, so the absolute numbers here are a
fraction of what a webview pays. The ratio between them is the part that
carries over, and the ratio is the thing being tested.

`App.bench.tsx` asks the same question of the whole app rather than the
transcript alone: when a token lands, does anything *else* redraw? The rail with
its list of conversations, the topbar and the model picker have nothing new to
say, and `App` reading only the fields it draws — rather than subscribing to the
whole store, which is the default — is what keeps them still:

```
whole store   1.70 ms mean, 1.12 ms min
by field      0.84 ms mean, 0.15 ms min
```

The floor is the interesting half. It is the transcript's own cost, which is
what a frame should cost when nothing else has moved.

## The app icon

Every icon the bundles use is generated from `app-icon.svg` at the repository
root, which is the same mark the rail draws — the `Logo` in
`src/components/icons.tsx`, on the same grid, with the colours resolved because
Finder and Explorer do not know what `var(--accent)` means.

```bash
pnpm tauri icon app-icon.svg     # regenerates src-tauri/icons/
```

Both halves of that matter. `src-tauri/icons/` held a flat purple square for
long enough to ship in a release, which nothing caught because no test can look
at a picture — so the mark and the icon are one file apart rather than two
unrelated drawings. The mobile output the generator also writes is deleted;
there is no Android or iOS project here to consume it.

On Windows the executable's icon does not come from the bundler at all.
`tauri-build` reads the first `.ico` in `bundle.icon` and embeds it as resource
`32512` while compiling, which is why a plain `cargo build --release -p
taurus-app` — what CI runs, producing `taurus-app.exe` — carries the icon
without any bundling step. Keep an `.ico` first in that list.

## The README's screenshots

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
is the whole of it. Commit only the images your change is actually about — a
different Chrome or a different font stack rewrites every file, and five
unrelated PNGs in a diff make the one that matters unreviewable.

A shot that has to *click* through something waits with a timer rather than
with `requestAnimationFrame`. That is the counter-intuitive half and it is
worth knowing before writing the next one: these run under Chrome's
`--virtual-time-budget`, and a frame loop that reschedules itself every frame
spends the entire budget without ever letting the fetch it is waiting on
land.

Some of these shots are the only check a behaviour has, and that is on purpose
rather than a gap. `query-run` presses **Run in Query** on a card in the
transcript and photographs where it lands — a lazily mounted pane, a tab
switch, and a query that comes back — which is a real browser doing the whole
round trip. `query-complete` types a half-written join into the query box and
photographs the completion list under the caret. Between them they are the only
check of anything measured from the DOM: jsdom has no layout, so it reports
every `scrollHeight` and every `getBoundingClientRect` as zero, and a unit test
of the box's auto-sizing or of where the list lands would be asserting numbers
the browser never produces. The mount tests prove the list has the right rows
in it; the PNG is what proves it is in the right place.

`palette` is the only check that a keyboard shortcut is bound at all. It opens
the box by dispatching the chord on `window` rather than by pressing anything,
which is the half no unit test can reach: jsdom can prove `isChord` agrees with
the label a row wears, and only a browser can prove the listener is on the
window to hear it. It sends the modifier `APPLE` says this machine uses — the
same constant the label is drawn from — because sending the other one would be
correctly refused, and the shot would then fail for the one reason that is not
a regression.

`permission-diff` earns its place twice over. Besides the dialog, it is the
only picture of a diff that has been coloured and marked: its hunk is one line
rewritten and one line added, so the same image shows the intra-line mark on
the pair that was rewritten and *no* mark on the addition that answers nothing.

Those scenes type into a React-controlled text box, which needs the value
written through the element's own prototype setter before the `input` event —
React keeps a tracker on the node and silently drops an event whose value it
believes it already has. `typeInto` in `scripts/screenshots/` does it, taking
the prototype off the element because the query box is a `<textarea>` and the
palette is an `<input>`; the mount tests do the same thing for the same reason.

`motion` is the odd one: a still image of a set of animations, which sounds
useless and is not. It cannot show that anything moves, and that is not what it
is for — it is the only check that the waveform renders where it should, that a
running row wears the treatment its category calls for, and that none of it has
landed on top of something else. It seeds `busy` and a genuinely in-flight call
rather than faking either, because the waveform's shape is chosen from the
category of the call that is running: the picture is only honest if a call
really is.

## Live checks

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

# MCP: repair the PATH the way the app does, connect, list tools, call one
# through the registry. Reports entries that would not parse, so a typo is named
# rather than passing in silence or taking its neighbours down with it.
cargo run -p taurus-mcp --example probe -- path/to/mcp.json

# Web: one real search, then fetch the first result it returns.
cargo run -p taurus-web --example probe -- ~/.taurus/search.json "rust async book"

# Reading a turn back to an agent that did not write it. Needs Ollama; writes
# only inside a temp directory. It plants a defect that is invisible from the
# hunk and visible from the doc comment two lines above it, then asserts the
# two claims a unit test cannot reach: that the reviewer read the file rather
# than only the diff, and that the workspace is untouched afterwards. A
# reviewer that "helpfully" restored the guard would have become a turn.
cargo run -p taurus-host --example review -- qwen3.6:27b

# What is inside the config a workspace wants to be trusted with. Needs no
# provider; reads a workspace and writes nothing. The answer to "why is this
# flagged", and the way to check the rules stay quiet on repositories you
# already trust — a scanner that fires on every folder is one people learn to
# click past. Run it over your own clones: a clean sweep is the expected
# result, and anything else is either a real finding or a rule to tighten.
cargo run -p taurus-host --example inspect -- .
cargo run -p taurus-host --example inspect -- ~/src/some-fresh-clone

# Themes: what the app makes of the ones on your disk. Needs no provider, reads
# `~/.taurus/themes` and writes nothing. The answer to "why is my theme not in
# the picker" — which layer each came from, which of the two palettes it can
# paint, which `--lk-*` tokens it actually sets, whether its logo was read, and
# every complaint the loader had. The app reports the problems of the theme in
# force and the picker reports the rest only while it is open; this reports all
# of them at once, without starting the app.
cargo run -p taurus-host --example theme        # global themes
cargo run -p taurus-host --example theme -- .   # and this workspace's

# What a sweep costs on a real workspace, and that it stays quiet when nothing
# changed. Needs no provider. Run it on something large before touching the
# caps in `sweep.rs` — every command pays this twice. It reports a turn's first
# command, its second, and a turn keeping no cache between them: the second
# should open almost nothing, and two numbers that match mean the cache is not
# working. `READ_THREADS` is a measured ceiling and not a core count — past a
# handful of readers a sweep gets slower, and by eight it is slower than one
# thread.
cargo run -p taurus-tools --example sweep -- .

# How well the index answers a question, as a number rather than by eye.
# Needs Ollama and an embedding model; reads the workspace and writes nothing.
# Fifteen questions phrased the way somebody asks them, each with the file that
# answers it, reported as the rank that file came back at. Run it, change
# something about chunking or ranking, run it again. It is the gate that
# `rerank_model` has been waiting for since it shipped, and it is what showed
# that structure-aware chunking retrieved worse than the line windows it would
# have replaced — see `docs/known-gaps.md`.
cargo run -p taurus-index --example retrieval -- . nomic-embed-text

# What searching your real transcripts costs, and what it finds. Needs no
# provider. It reads `~/.taurus/sessions` and writes nothing. Two numbers, and
# the gap between them is the point: a query that matches nothing pays only the
# prefilter — every file read, none parsed — and a query that hits pays to
# rebuild what matched. If those are close on a large history the prefilter in
# `sessions::mentions` is not working, and the palette will feel a word behind
# the typing. It prints each hit with the mark the palette would draw, so a
# wrong offset shows up here rather than only in the window.
cargo run -p taurus-host --example search -- "something you said"

# A command that outlives the call that started it. Needs no provider. It
# starts one in a throwaway workspace, shows it running while nothing has
# changed yet, reads it back the moment it exits, and undoes it — proving the
# pre-image is the file as it stood before the command ran rather than after.
# Then it stops one that would never have stopped on its own. Last, the part
# the unit tests can only assert one moment of: a chatty command read by the
# window on a timer *while it runs*, and then read in full by `check_command`
# afterwards. Every line has to appear in both. If one is missing from the
# second, the two cursors have collapsed into one and a pane drawing a build is
# emptying the buffer the model was going to read.
cargo run -p taurus-tools --example background

# Memory, across two conversations: one turn leaves a note, and a second one —
# a new session, told not to read anything — answers from it. The second half is
# the check that matters. A note that is written and never carried is a file
# nobody reads, and nothing in the first turn's output would say so.
taurus run --model qwen3.6:27b "Read api.py. The bug on line 2 is real but do not fix it now. \
  Leave a note so the next conversation knows about it, then stop."
taurus notes list
taurus run --model qwen3.6:27b "Without reading any files: is there anything I should know \
  about this project before I start?"

# A turn recorded, read back as a diff, and committed — against the git binary
# on this machine, in a repository it builds and throws away. Needs no
# provider. This is the only check that proves the reasons a file was left out
# of a commit are the true ones.
cargo run -p taurus-host --example turn

# An image, attached and answered. Prints the capability it probed, then whether
# the model named both colours in the right order — a model that received no
# image still answers confidently, so "it replied" is not evidence.
cargo run -p taurus-host --example vision -- gemma4:12b
cargo run -p taurus-host --example vision -- llama3.2:latest   # refused, and why

# Load, profile, and page a real data file, and say what each one cost. Needs
# no provider. The unit tests read a five-row CSV; what they cannot show is
# behaviour on a file somebody actually has, which is the only thing that
# decides whether this is any good. Run it on something large before touching
# the caps in `df.rs` or the two-pass arrangement in `profile`.
#
# Three numbers, and what each one means is in the example's own header:
# `schema` must stay flat as the file grows, `profile` is a full pass and is
# allowed to be slow, and `page` must be flat in the *offset* — which is why it
# is measured at row 0 and again at the end.
cargo run -p taurus-data --example probe -- ~/data/interactions.csv

# With a query, which is the other half. The table is named the way
# `load_dataset` names it, so the SQL here is the SQL you would type in the
# pane — and handing it a write is how the refusal gets checked against a real
# file rather than a fixture.
cargo run -p taurus-data --example probe -- ~/data/interactions.csv \
  "SELECT category, count(*) AS n FROM interactions GROUP BY 1 ORDER BY n DESC"
cargo run -p taurus-data --example probe -- ~/data/interactions.csv \
  "COPY interactions TO '/tmp/escaped.parquet'"   # must refuse, and write nothing

# A recipe, which is the only thing in this crate that writes a file the user
# can see. Same argument as the probe and more so: a step that drops every row
# because a column arrived as text, or a join that fans out because a key is
# not unique, are properties of real data and cannot happen in a fixture. Watch
# the delta column — that is the whole reason a run is reported per step.
cargo run -p taurus-data --example recipe -- .taurus/recipes/purchases.sql
cargo run -p taurus-data --example recipe -- .taurus/recipes/sneaky.sql
#   ^ with a `COPY … TO` in step 2: must refuse by step number and write nothing,
#     not even the output the earlier steps had already produced.

# The same run with the harness around it — the catalog, the path guard, and
# the output being loaded as a dataset afterwards. Needs no provider either,
# which is the point: a recipe is deterministic, so it belongs in a make target.
taurus data list -w ~/data
taurus data run purchases -w ~/data

# The half the probe cannot check: whether a model reaches for these at all.
# The failure worth catching is a model answering a question about a CSV with
# `read_file`, which costs a whole context window and answers nothing — so what
# this proves is the *absence* of a tool call, and no unit test can see that.
# Ask a plain question, with nothing in it naming a tool.
taurus run -w ~/data "What is in interactions.csv, and which event type is most common?"

# And one the profile cannot answer, which is what `query_data` is for. The
# thing to watch is that it writes SQL rather than reaching for a shell and an
# awk pipeline — the tool description is the only thing steering that.
taurus run -w ~/data "Which three categories have the highest share of refunds?"

# And the recipe half: whether a model writes one rather than answering once and
# forgetting. What to watch is that it reaches for `write_file` into
# `.taurus/recipes` and then `run_recipe`, rather than running four `query_data`
# calls and pasting the numbers into the answer.
taurus run -w ~/data "Build me a purchases table: drop duplicates, keep only \
  purchases with a rating, and rank each user's by price. Save it so I can re-run it."

# Index a real workspace and ask it real questions. Proves the second pass is
# near-free, which is the property the whole design turns on, and prints
# rankings a reader of this repository can check by eye. Run it on something
# large before changing the caps in `store.rs`.
ollama pull nomic-embed-text
cargo run -p taurus-index --example probe -- . nomic-embed-text
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
taurus mcp                      # non-zero exit if a server failed to connect or would not parse
taurus key status               # where each provider's API key comes from
```

`skills check`, `agents check`, and `mcp` are meant for CI on a repository that
ships its own `.taurus` directory.

## The Windows ConPTY runtime

A Windows build sideloads Microsoft's redistributable ConPTY next to the
executable, because the system's console host shows a window when the process
asking for it has none — which is every release build. `scripts/conpty.mjs`
fetches it, and `tauri.conf.json` calls that script from `beforeBuildCommand`,
so a Windows build needs nothing done by hand.

```bash
node scripts/conpty.mjs                   # no-op off Windows
TAURUS_CONPTY_FORCE=1 node scripts/conpty.mjs   # run it anywhere, to test it
```

The force flag exists because the one platform that needs this is the one
platform it cannot be tested on before a release: with it, the download, both
hash checks and the extraction can be exercised on a Mac or a Linux box. The
zip is read by the script itself rather than by a tool or a dependency, so
there is nothing platform-specific left in that path.

The package version and the SHA-256 of the archive and of all three files are
pinned in the script. Bumping it means changing four hashes — download the new
package, check the bytes, and paste them in. A mismatch fails the build and
writes nothing, which is the point: this is the one script whose whole job is
being sure about bytes nobody in this project compiled.

The files are gitignored. They are 2.2 MB that we neither build nor own, and
committing them would put a fresh copy in history on every bump.

**How to tell it worked.** The app logs at startup whether it found the runtime
beside the executable. That log line is the only signal there is — packaged
wrongly, everything still works, except that a console window appears on every
`pty: true` command, which nothing but a person on Windows running an installed
build will ever see.

## Cutting a release

```bash
node scripts/version.mjs set 0.2.0     # package.json, Cargo.toml, tauri.conf.json, Cargo.lock
git commit -am "release: 0.2.0"
git tag v0.2.0 && git push origin main v0.2.0
```

The version lives in four places and only one of them is a file the tag can be
checked against by eye. `set` writes all of them at once, including the fifteen
workspace entries in `Cargo.lock` — a bump that stops at `Cargo.toml` leaves a
lock the next `cargo` command rewrites underneath the build. `.github/workflows/release.yml`
runs `node scripts/version.mjs check "$TAG"` before it compiles anything, so the
mismatch costs twenty seconds rather than three platforms' worth of build:

```bash
node scripts/version.mjs check         # the files agree with each other
node scripts/version.mjs check v0.2.0  # …and with this tag
```

That check earns its place because the failure it prevents is invisible in a
green run. `tauri-action` takes the release name from the tag and the bundle
filenames from `tauri.conf.json`, so a tag pushed against an unbumped tree
publishes a release called v0.2.0 in which every downloadable file is named
`Taurus_0.1.0_…`.

The tag builds macOS (one universal `.dmg` covering both architectures), Windows,
and Linux into a **draft** release, and publishes it only once all three have
uploaded. Draft-until-complete is the point: published-by-default means the first
platform to finish makes the release visible and the other two upload into
something the world can already see — and macOS, building two architectures, is
last by a wide margin. If one platform fails, the draft stays a draft with the
others attached, and re-running the workflow adds the missing one to that same
draft rather than opening a second.

Release notes are generated from the commits since the previous tag. That is a
floor, not a substitute for writing them: the draft is editable for as long as
the slowest platform is still building.

To exercise the workflow without cutting a tag, run it from the Actions tab —
`workflow_dispatch` builds all three platforms, publishes nothing, and leaves the
bundles as workflow artifacts to download and open.

Two things a release does **not** do yet. Nothing is code-signed — there are no
Apple Developer or Windows certificates wired up, so a downloaded `.dmg` is
stopped by Gatekeeper and the `.msi` by SmartScreen until the user works around
it. And no release carries the `taurus` CLI; `cargo install --path
crates/taurus-cli` from a clone is still the only way to get it.
