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

## Styling

Styling lives in two files:

- `src/styles.css` names the Lukeus palette once at the top, then writes
  everything in roles: the window at `--bg`, a panel raised off it at
  `--bg-raised`, anything inset back into a panel at `--bg-sunken`.
- `src/tailwind.css` hands those roles to Tailwind 4 as its theme and removes
  Tailwind's own defaults.

So `bg-raised` compiles to `background-color: var(--bg-raised)`: the same
declaration a hand-written rule makes, from the same token, resolved at the
element instead of frozen at build time.

Nothing else has to know Tailwind is there. The light theme restates only the
raw `--lk-*` tokens, a custom theme writes fourteen inline properties on
`<html>`, and `contrast.test.ts` measures the pairs the app puts on screen. A
utility and a rule are two spellings of one decision.

**There is no `dark:` variant in this app and there should never be one.** The
role ladder exists so a token means the same thing under both themes. A
`dark:` prefix makes that decision twice, the second time out of the theme
editor's reach. `tailwind.css` does define a `light:` variant, a marked escape
hatch for the rare rule the ladder doesn't cover, like a shadow that needs
less black on a light ground. Reaching for it should feel like reaching for
something.

An alias that only restates its declaration earns nothing, so there's no
`.mono` or `.muted`: `font-mono` and `text-dim` say the same thing in the same
tokens. `.spacer` stays, though it means exactly `flex: 1`. `flex-1` says what
the browser does. `spacer` says why the empty div is there, which the
declaration can't.

Tailwind's palette, type scale, radii, easings and shadows are cleared, not
just unused. `bg-gray-800` compiles to nothing, and so do `text-sm` and
`rounded-2xl`. The vocabulary is the app's own:

- surfaces: `canvas`, `rail`, `raised`, `sunken`, `elevated`, `hover`,
  `active`, `row`
- text: `ink`, `dim`, `faint`
- lines: `line`, `rule`, `thread`
- signals: `accent`, `ok`, `warn`, `danger`
- a type scale named by size: `text-12-5` is the app's control size, `text-10`
  its mono micro-labels

### Converting a component

The worked example is `src/components/Rail.tsx`, which uses utilities
throughout. Its conversion deleted four hundred lines of `styles.css` and came
out pixel-identical. That's the bar. Settings is the second, more instructive
example (see "What belongs to a component").

1. **Write every property the element ends up with, except the primitives.**
   `button`, `input` and friends keep base rules as the app's own reset, so a
   converted button spells out only its deltas. Everything else on the element
   should be readable from its class list.
2. **State becomes a `data-` attribute, not a class.** Use `data-current`,
   `data-armed`, `data-shut`, styled with `data-current:bg-active`, or for
   descendants, `group` plus `group-data-current:text-ink`. Render
   `data-current={current || undefined}` so the attribute is absent, not
   `"false"`.
3. **Keep the semantic class name.** It costs nothing, tests and `data-tip`
   tooling find the element by it, and it says what the element *is*. The
   utilities only say what it looks like.
4. **Move the rationale with the decision.** The comments in `styles.css` are
   most of its value. Deleting a rule without landing its comment beside the
   new class list is a net loss, whatever the diff says.
5. **Delete the rules you replaced, and re-run `pnpm screenshots`.** A
   zero-pixel diff is the proof. Anything else is a decision you didn't know
   you were making. If the surface has no baseline, add one. The providers tab
   got `docs/screenshots/providers.png` and four IPC stubs for this reason:
   most of the Settings form had never been photographed.

   Six baselines are unreliable, and trusting them will waste an afternoon:

   - `canvas.png` and `motion.png` don't reproduce on every machine. They
     differ by the same amount at a clean `HEAD`: environment drift, not you.
   - `app-light`, `palette`, `query-run`, `query-complete`, `mcp-catalog` and
     `canvas-conflict` are flaky between runs. A "Jump to the end" pill and
     some mid-animation frames come and go.

   Re-run before believing any of the six.

### What belongs to a component

Not whatever its name says. This is the biggest trap in the migration, and
every guess costs a silent regression.

Settings looked like 1,714 lines of its own markup. Almost none of it was:

- The drawer wears `card`, `drawer`, `micro`, `spacer` and `hint`, which
  fifteen to twenty-seven other files also wear.
- Less obviously, it wears `settings-field`, `settings-actions`,
  `settings-problem` and `settings-check`. The prefix says Settings owns them,
  and it's wrong. The theme editor lays out its fields with the first, and
  eleven panels say why something failed with the third.

Deleting those rules because of the prefix unstyled all eleven panels, and
nothing failed.

So before you delete a rule, count the files that write its class:

```bash
grep -rlE 'className=\{?[`"][^`"]*\bsettings-problem\b' --include='*.tsx' src
```

- **One file:** the component owns it, and it converts.
- **More than one:** it's a shape several components agreed on, and that's
  what a rule is for. Leave it, whatever it's called.

Past a certain reach, a shared rule should be a component. Eleven panels
wearing `settings-problem` wasn't a styling problem but a missing abstraction.
That sentence is `<Problem>` and `<Problems>` in `src/components/Problem.tsx`,
and the eleven import it instead of knowing which drawer happened to write it
first. Three `settings-*` names are shared and misnamed. They're worth
renaming on their own, not inside a conversion.

Drawers follow the same pattern. Ten panels were writing out the same `Modal`,
the same `<aside className="drawer">` and the same header, and they had
drifted. Three spelled the dismiss as the word "Close" and seven as a ✕, so
one control in one corner read two ways depending on which panel was open.
`src/components/Drawer.tsx` writes that shape once:

- `<Drawer title onClose panel actions overlay>` for the seven that fit
- `<DrawerHead>` for the five that share only the title bar

The extraction barely changed the line count; most of its diff was
re-indentation. What it's for is one line that was in nine of the ten and is
easy to leave out of the tenth:

```jsx
onClick={(e) => e.stopPropagation()}
```

`Modal` dismisses on any click that reaches the scrim. Without that line,
clicking any patch of drawer that isn't a control closes the drawer. No type
catches it and no test notices. The panel just shuts under somebody's cursor.

The `overlay` slot is the same kind of knowledge. An editor opened from a
drawer has to render *inside* that drawer's scrim and outside its panel.
That's how `Modal` decides whether Escape belongs to it or to whatever is on
top of it.

The extraction turned up two more things, both about order:

- **A rule's position in the file can be load-bearing.** `.hint` looks like a
  primitive, so it was moved up to sit with `.micro` and `.mono`. But the MCP
  drawer opens with `class="drawer-intro hint"` and needs `hint` to win. Both
  have the same specificity, so only source order decides. After the move, the
  intro read a point larger and a shade brighter, and every row under it
  shifted three pixels down. `.hint` stays put, with a comment saying why.
- **A rule that never applied looks exactly like one that does.**
  `.index-bar { width: 100% }` lost to `.plan-bar { width: 64px }` seven lines
  below it, so the index progress bar had been a 64px stub. As a utility it
  wins, as the rule always said. Converting a class can turn a dead rule live.

### What the tests hold

`src/styles.test.ts` reads the stylesheet *and* every `className` in the tree,
because half the styling decisions live in the markup.

- **The ladder** (2, 4, 6, 8, 10, 12, 16, 20, 24, 32) holds for padding,
  margin and gap, whether written `padding: 12px` or `p-3`. Sub-4px is exempt.
  So is `auto`, which isn't a distance.
- **No class name may be one a utility also claims.** `.table-row { display:
  grid }` taught this. `table-row` is a CSS `display` value, so Tailwind
  generates a utility for it, and a utility outranks the stylesheet's layer by
  design. Every table in the app silently stopped being a grid, while the
  typecheck and a thousand tests passed. That's why the class is `.table-line`.
- **No second palette**, and no size or radius from a scale this app doesn't
  have. A cleared name compiles to nothing, and "the text is the wrong size" is
  the slowest way to find out a class name was wrong.
- **The one destructive control in the rail keeps a 28px target**, read from
  its class list, where the two numbers live.
- **No shared class name is left without a rule or a utility behind it.** Every
  class the markup writes must be one or the other. A name that's neither
  renders as nothing. The check covers only names used by more than one file,
  since a converted component can keep a ruleless name of its own: tests find
  `rail-row` and `settings-provider` by name, with every declaration in the
  utilities beside them. This check catches a deleted rule and names the files
  still wearing it.

### The cascade, once

`tailwind.css` declares the order: `theme, base, vendor, taurus, components,
utilities`. `taurus` is `styles.css`, wrapped at both ends. `vendor` is xterm's
own sheet, imported through `src/vendor.css`.

Layering the hand-written stylesheet isn't cosmetic. Unlayered CSS beats every
layer regardless of specificity, so `button { padding: 6px 12px }` would
quietly beat `px-3 py-1.5` on a converted button. The conversion would look
finished and change nothing on screen.

`styles.css` imports `tailwind.css` itself, not the entry point, because there
are two entry points: the app and the screenshot harness. When only one of
them had both files, every image came out of a rail with none of its layout.
One import brings the other with it, in the right order.

Preflight is deliberately not imported. The app already has its reset:
box-sizing, margins, the button and input primitives, the scrollbars. A second
reset under it would move pixels in twenty-one baselines just to land where
the app already is.

## Drawing a token

```bash
pnpm bench     # src/components/Transcript.bench.tsx
```

Read whether the rows climb, not any one row.

A transcript grows all day. A renderer that redraws the whole conversation for
every token gets slower as the conversation runs, the kind of slowness people
report as "it was fine this morning". The four cases differ only in how much
conversation sits *above* the turn being streamed into, so a healthy result is
four numbers that stay flat:

```
· under  1 turn  of history    0.111 ms
· under  5 turns of history    0.093 ms
· under 20 turns of history    0.121 ms
· under 50 turns of history    0.159 ms
```

Before per-turn memoization, the transcript redrew the whole conversation, and
the same four read 0.173, 0.395, 1.449, and 3.406: a twenty-fold climb, and
22x the cost at fifty turns.

Two things keep it flat, and both are load-bearing:

- **Turns carry their identity forward.** For any turn whose entries are the
  same objects, `reuse` in `Transcript.tsx` hands back the object it built last
  time, which gives the per-turn memo something to compare. The reducer never
  rewrites an entry it didn't touch, so this is exact, not a heuristic.
  `Transcript.test.tsx` covers it, because a refactor that dropped it would
  cost nothing visible and quietly bring back the full redraw.
- **The transcript holds its callbacks steady itself.** Callers naturally
  write `onAnswer={() => …}` inline, and `DelegateTranscript` does. Without
  this, every turn would get a new function on every token, undoing all of the
  above. Owning it beats making every caller learn a rule.

Underneath both, the store batches a frame of stream events at a time
(`batchEvents`) instead of writing per token, so a fast local model produces
thirty-odd renders a second, not hundreds.

The other axis is the reply itself. Parsing the whole reply every frame would
make each frame dearer than the last, and the reply as a whole would cost the
square of its length. So `blocks` in `Markdown.tsx` cuts the text at blank
lines nothing can reach across and memoizes each piece on its own text. A
frame parses only the paragraph being written. The bench's second group is
one frame at three reply lengths:

```
· 10 paragraphs in      1.10 ms
· 50 paragraphs in      1.08 ms
· 200 paragraphs in     1.40 ms
```

Parsed whole, the same three measured 3.93, 14.78, and 58.63 ms. The bench
draws a finished entry, because an open one is throttled to a parse every
60 ms and a bench never waits that long. Each throttle tick pays exactly what
one of these iterations pays.

The cut relies on one property only: parsed apart, the pieces draw what the
whole would have. `Markdown.test.tsx` checks this over documents full of the
constructs that reach furthest: loose lists, fences with blank lines inside,
indented code, a reference definition. `blocks` refuses to cut at all where a
definition or a raw HTML block could reach across a blank line.

jsdom doesn't lay out or paint, so the absolute numbers are a fraction of what
a webview pays. The ratio carries over, and the ratio is what's being tested.

`App.bench.tsx` asks the same question of the whole app: when a token lands,
does anything *else* redraw? The rail and its conversation list, the topbar
and the model picker have nothing new to show. They stay still because `App`
reads only the fields it draws, instead of subscribing to the whole store (the
default):

```
whole store   1.70 ms mean, 1.12 ms min
by field      0.84 ms mean, 0.15 ms min
```

The floor is the interesting half: the transcript's own cost, which is what a
frame should cost when nothing else has moved.

## The app icon

Every icon the bundles use is generated from `app-icon.svg` at the repository
root. It's the same mark the rail draws (the `Logo` in
`src/components/icons.tsx`, on the same grid), with the colors resolved
because Finder and Explorer don't know what `var(--accent)` means.

```bash
pnpm tauri icon app-icon.svg     # regenerates src-tauri/icons/
```

The mark and the icon are one file apart, not two unrelated drawings, for a
reason: `src-tauri/icons/` held a flat purple square long enough to ship in a
release. Nothing caught it, because no test can look at a picture.

Delete the mobile icons the generator also writes. There's no Android or iOS
project here to use them.

On Windows, the executable's icon doesn't come from the bundler at all.
`tauri-build` embeds the first `.ico` in `bundle.icon` as resource `32512`
while compiling. So a plain `cargo build --release -p taurus-app`, which is
what CI runs to produce `taurus-app.exe`, carries the icon with no bundling
step. Keep an `.ico` first in that list.

## The README's screenshots

```bash
pnpm screenshots     # rewrites docs/screenshots/*.png
```

The images are the real frontend (the real `App`, store and stylesheet),
served on its own and driven in headless Chrome, with fixtures answering the
Tauri IPC bridge. They aren't photographs of the running desktop app: there's
no window chrome, and the conversation is canned.

That trade is deliberate, for the same reason as the app icon above. Nothing
can check a hand-taken screenshot, so it goes stale silently and ends up
showing an app nobody runs any more. One regenerated from the components in
`src/` can't drift further than the last person who ran the command.

The fixtures are the `UiEvent` stream a turn emits, folded by the store's own
reducer, not hand-built view state. A screenshot built around the components
would look right while the code feeding them was broken.

Regenerate after a visible UI change, and eyeball the result. This is the only
check in the repository that looks at pixels, and the check is you reviewing
the PNG in the diff. Commit only the images your change is about. A different
Chrome or font stack rewrites every file, and five unrelated PNGs in a diff
make the one that matters unreviewable.

A shot that has to *click* through something waits on a timer, not
`requestAnimationFrame`. That's counter-intuitive, so know it before you write
the next one. The shots run under Chrome's `--virtual-time-budget`, and a
frame loop that reschedules itself every frame spends the whole budget without
ever letting the fetch it's waiting on land.

Some shots are the only check a behavior has. That's on purpose, not a gap.

- `query-run` presses **Run in Query** on a card in the transcript and
  photographs where it lands: a lazily mounted pane, a tab switch, and a query
  that comes back. That's a real browser doing the whole round trip.
- `query-complete` types a half-written join into the query box and
  photographs the completion list under the caret.

Between them, they're the only check of anything measured from the DOM. jsdom
has no layout, so it reports every `scrollHeight` and `getBoundingClientRect`
as zero. A unit test of the box's auto-sizing, or of where the list lands,
would assert numbers the browser never produces. The mount tests prove the
list has the right rows. The PNG proves it's in the right place.

`notes` and `notes-diagram` are the only pictures of the notes pane, and the
only check of two things in it:

- **The prose editor wraps**, unlike the canvas's. With no layout, jsdom can
  prove the text is in the box, but nothing about where its lines break.
- **The Mermaid reader's drawing.** Unit tests check it down to the last stage
  and edge, but none can say whether the picture is *drawn* where its own
  arrows point.

The second shot paid for itself right away. It showed an edge inside one
subgraph arriving dashed, so a Mermaid chain drew its happy path in the
treatment reserved for failures.

`sketch` and `notes-sketch` are the only pictures of Excalidraw in the app,
and the only check of three things invisible to a test:

- its fonts arrive from this origin, not a fallback face
- none of the app's own `button` and `input` rules reach its toolbar
- its menu is the trimmed one

`notes-sketch` found a bug on its first run: every embed drew at twice its
size. Excalidraw's exporter fills a missing `exportScale` from the device pixel
ratio, and the harness renders at 2×, like a Retina screen.

In both Vite configs, `scripts/excalidraw-assets.mjs` handles Excalidraw's
fonts and translations:

- It serves the fonts from `node_modules` in the dev server and copies them
  into `dist/excalidraw/fonts/` in a build, because the CSP refuses the CDN
  they'd otherwise come from.
- It points Excalidraw's fallback for every face at this origin too, because
  each CDN fallback was reported as a refused font on every sketch.
- It replaces every translation but English with an empty module. The editor
  is pinned to English, and the other fifty-two are a megabyte of files
  nothing reads.

These scenes wait for the editor to appear before pressing **Read**. That's the
same virtual-time trap again: two spinning waits in a row exhaust the budget
between them, and the shot comes out as an empty pane, not a failure anyone
would notice. Gate each step on what the last one fetched.

`palette` is the only check that a keyboard shortcut is bound at all. It opens
the box by dispatching the chord on `window`, not by pressing a key: the half
no unit test can reach. jsdom can prove `isChord` agrees with the label a row
wears, but only a browser can prove the listener is on the window to hear it.
The shot sends the modifier `APPLE` says this machine uses, the same constant
the label is drawn from. The other one would be correctly refused, and the
shot would fail for the one reason that isn't a regression.

`permission-diff` earns its place twice. Besides the dialog, it's the only
picture of a colored, marked diff. Its hunk rewrites one line and adds one,
so the same image shows the intra-line mark on the rewritten pair and *no*
mark on the addition, which answers nothing.

Those scenes type into a React-controlled text box, so the value has to go
through the element's own prototype setter before the `input` event. React
keeps a tracker on the node and silently drops an event whose value it thinks
it already has. `typeInto` in `scripts/screenshots/` does this, taking the
prototype from the element because the query box is a `<textarea>` and the
palette an `<input>`. The mount tests do the same, for the same reason.

`motion` is the odd one: a still image of animations. That sounds useless, but
isn't. It can't show that anything moves, and that's not its job. It's the
only check that:

- the waveform renders where it should
- a running row wears the treatment its category calls for
- none of it lands on top of something else

It seeds `busy` and a real in-flight call instead of faking either, because
the waveform's shape comes from the running call's category. The picture is
only honest if a call really is running.

## Live checks

These run against a real Ollama server. They're the fastest way to check your
change didn't break what unit tests can't reach.

```bash
# One provider, one turn, one tool call.
cargo run -p taurus-provider-ollama --example ollama-smoke -- qwen3.6:27b
cargo run -p taurus-provider-ollama --example ollama-smoke -- gemma3      # prompted fallback

# The OpenAI adapter, against Ollama's own /v1 endpoint.
cargo run -p taurus-provider-openai --example openai-smoke -- llama3.2:latest

# The hosted adapters. Each prints the capabilities it probed before the turn,
# which is the half of these two that has no local equivalent.
ANTHROPIC_API_KEY=… cargo run -p taurus-provider-anthropic --example anthropic-smoke -- claude-opus-5
GEMINI_API_KEY=…    cargo run -p taurus-provider-gemini    --example gemini-smoke -- gemini-2.5-pro

# The whole harness: read files, write a file, report what happened.
cargo run -p taurus-core --example e2e -- qwen3.6:27b

# Skill authoring: propose, validate, save, rediscover.
cargo run -p taurus-skills --example synthesis -- qwen3.6:27b

# Delegation: define a scoped agent, delegate to it, prove it stayed in scope.
cargo run -p taurus-agents --example delegate -- qwen3.6:27b

# MCP: repair the PATH the way the app does, connect, list tools, call one
# through the registry. Reports entries that would not parse, so a typo is named
# rather than passing in silence or taking its neighbours down with it.
cargo run -p taurus-mcp --example mcp-probe -- path/to/mcp.json

# Web: one real search, then fetch the first result it returns.
cargo run -p taurus-web --example web-probe -- ~/.taurus/search.json "rust async book"

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

# Notes: what the app makes of the notebooks on your disk, and what a save does
# under a race. Listing and reading need no provider and write nothing;
# `--check` writes, and only inside a temp directory it makes and reports.
# The listing is the half a test cannot have — a real directory somebody else
# edits, holding a note added by hand, a file with the wrong extension, or a
# name the filesystem took and this would refuse, which is how a note ends up
# listed and unopenable. `--check` runs the compare-and-swap for real: read,
# write behind the editor's back, save with the stale stamp, watch it refused
# with the other version in hand, then save with the stamp the refusal returned.
cargo run -p taurus-host --example notes                     # both notebooks
cargo run -p taurus-host --example notes -- .                # and this workspace's
cargo run -p taurus-host --example notes -- . 'Auth redesign'
cargo run -p taurus-host --example notes -- --check

# What a sweep costs on a real workspace, and that it stays quiet when nothing
# changed. Needs no provider. Run it on something large before touching the
# caps in `sweep.rs` — every command pays this twice. It reports the first
# command in a workspace, the one after it, and a sweep keeping no cache: the
# second should open almost nothing, and two numbers that match mean the cache
# is not working. The second is also what the next turn's first command costs,
# because the host holds the cache for the workspace rather than for a turn. `READ_THREADS` is a measured ceiling and not a core count — past a
# handful of readers a sweep gets slower, and by eight it is slower than one
# thread.
cargo run -p taurus-tools --example sweep -- .

# What grep costs on a tree too big to judge by eye. Needs no provider, and
# writes only inside a temp directory it makes and removes. Release, because
# the regex crate built for debug is a different program. Three numbers: a
# pattern found nowhere, which reads every file; one capped at the result
# limit; and big files whose one match sits on their last line. Measured when
# grep went parallel and stopped testing a matched file line by line: 94.8 ms
# to 49.8, 3.8 to 3.1, and 28.3 to 6.3.
cargo test --release -p taurus-tools --lib grep_cost -- --ignored --nocapture

# What read_file costs on a log too large to read whole, which is where a
# spilled command's output sends the model. Same rules as the one above: no
# provider, a temp directory, release. Four reads of one 200 MB file — its
# first window, its middle, near its end, and past its end — and they should
# cost the same, because each still counts every line for the range note.
# Measured when reads began streaming and keeping only the window: 64 to 67 ms
# each, to 25 or 26, and a read holds the window rather than the file.
cargo test --release -p taurus-tools --lib read_file_cost -- --ignored --nocapture

# What a command's output costs the process running it. Needs no provider, and
# writes only inside temp directories it makes. Two commands that each print
# 100 MB — ordinary lines, and one line with no newline in it — through
# run_command with a screen attached. The number to watch is the maximum
# resident set `time` reports (`-l` on macOS, `-v` with GNU time): output the
# model will only ever see the two ends of should not be held whole. Measured
# when output began to be read in pieces and written out past 8 MB: 228 MB to
# 16 MB for the lines, and 406 MB to 15 MB for the single line, whose screen
# was sent one 100 MB message before and nothing over 8 KB after. Most of the
# single line's three seconds is `tr` itself.
cargo build --release -p taurus-tools --example output
/usr/bin/time -l target/release/examples/output lines
/usr/bin/time -l target/release/examples/output one-line

# What building one request costs on a long conversation with pictures in it:
# sixty messages, three 2 MB screenshots and twenty 8 KB tool results, about
# 6 MB. Release, for the reason grep's is. One number per copy the request
# makes: the history cloned for the attempt, 110 µs; the wire body built from
# it, 240 µs; and the body serialized, 2.04 ms. The first two look avoidable
# and are a sixth of the whole — about 9 ms over a 25-iteration turn — so the
# adapters still build their wire bodies as JSON trees rather than borrowing
# from the history. Run this before deciding otherwise.
cargo test --release -p taurus-provider-anthropic --lib request_build_cost -- --ignored --nocapture

# What search_code spends in this process on a large index: 6,000 passages of
# 768 dimensions, about 24 MB. Release, as above. Measured: reading the index
# off disk 7.6 ms, copying its entries 0.7 ms, decoding every vector 6.6 ms,
# and the search that decodes and scores them 9.6 ms. That is why the decoded
# vectors are not kept between searches: keeping them would save about 14 ms of
# a search, and cost a cache threaded through the refresh and invalidated on
# every save.
cargo test --release -p taurus-index --lib search_code_cost -- --ignored --nocapture

# What the context estimate costs over a long session: 300 messages, a hundred
# write_file calls with 1 KB of arguments and a 4 KB result each. The loop
# walks the whole history two or three times an iteration, so this grows with
# the session. Measured when tool arguments stopped being serialized into a
# string only to be measured: 1,000 walks went from 286.7 ms to 73.6 ms, with
# every estimate unchanged.
cargo test --release -p taurus-core --lib estimate_cost -- --ignored --nocapture

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
# allowed to be slow, and `page` is measured at row 0 and again at the end —
# the first counts the file once per version of it, and the gap to the second
# is the offset, which a CSV or NDJSON file reads its way to.
cargo run -p taurus-data --example data-probe -- ~/data/interactions.csv

# With a query, which is the other half. The table is named the way
# `load_dataset` names it, so the SQL here is the SQL you would type in the
# pane — and handing it a write is how the refusal gets checked against a real
# file rather than a fixture.
cargo run -p taurus-data --example data-probe -- ~/data/interactions.csv \
  "SELECT category, count(*) AS n FROM interactions GROUP BY 1 ORDER BY n DESC"
cargo run -p taurus-data --example data-probe -- ~/data/interactions.csv \
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
cargo run -p taurus-index --example index-probe -- . nomic-embed-text
```

The drawn results have no example of their own. The check worth making is that
a model reaches for them unprompted, and that what it sends survives the trip.
One turn does both:

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
executable. The system's console host shows a window when the process asking
for it has none, which is every release build. `scripts/conpty.mjs` fetches
it, and `tauri.conf.json` calls that script from `beforeBuildCommand`, so a
Windows build needs nothing done by hand.

```bash
node scripts/conpty.mjs                   # no-op off Windows
TAURUS_CONPTY_FORCE=1 node scripts/conpty.mjs   # run it anywhere, to test it
```

The one platform that needs this is the one you can't test it on before a
release. The force flag lets you exercise the download, both hash checks and
the extraction on a Mac or a Linux box. The script reads the zip itself, with
no tool or dependency, so nothing in that path is platform-specific.

The script pins the package version and the SHA-256 of the archive and all
three files. Bumping it means changing four hashes: download the new package,
check the bytes, paste them in. A mismatch fails the build and writes nothing.
That's the point: this is the one script whose whole job is being sure about
bytes nobody in this project compiled.

The files are gitignored. They're 2.2 MB we neither build nor own, and
committing them would add a fresh copy to history on every bump.

**How to tell it worked.** At startup, the app logs whether it found the
runtime beside the executable. That log line is the only signal. Packaged
wrong, everything still works, except a console window appears on every
`pty: true` command, which only a person on Windows running an installed build
will ever see.

## Cutting a release

```bash
node scripts/version.mjs set 0.2.0     # package.json, Cargo.toml, tauri.conf.json, Cargo.lock
git commit -am "release: 0.2.0"
git tag v0.2.0 && git push origin main v0.2.0
```

The version lives in four places, and only one is a file you can check the
tag against by eye. `set` writes all four at once, including the fifteen
workspace entries in `Cargo.lock`. A bump that stops at `Cargo.toml` leaves a
lock the next `cargo` command rewrites underneath the build.

`.github/workflows/release.yml` runs `node scripts/version.mjs check "$TAG"`
before compiling anything, so a mismatch costs twenty seconds, not three
platforms' worth of build:

```bash
node scripts/version.mjs check         # the files agree with each other
node scripts/version.mjs check v0.2.0  # …and with this tag
```

The check matters because the failure it prevents is invisible in a green run.
`tauri-action` takes the release name from the tag and the bundle filenames
from `tauri.conf.json`. So a tag pushed against an unbumped tree publishes a
release called v0.2.0 in which every downloadable file is named
`Taurus_0.1.0_…`.

The tag builds macOS (one universal `.dmg` covering both architectures),
Windows, and Linux into a **draft** release, and publishes it only once all
three have uploaded. Otherwise the first platform to finish would make the
release visible, and the other two would upload into something the world can
already see. macOS builds two architectures, so it's last by a wide margin.

If one platform fails, the draft stays a draft with the others attached.
Re-running the workflow adds the missing one to that draft instead of opening
a second.

Release notes are generated from the commits since the previous tag. That's a
floor, not a substitute for writing them: you can edit the draft for as long
as the slowest platform is still building.

To exercise the workflow without cutting a tag, run it from the Actions tab.
`workflow_dispatch` builds all three platforms, publishes nothing, and leaves
the bundles as workflow artifacts you can download and open.

Two things a release does **not** do yet:

- **Code signing.** No Apple Developer or Windows certificates are wired up.
  Gatekeeper stops a downloaded `.dmg`, and SmartScreen stops the `.msi`, until
  the user works around it.
- **The `taurus` CLI.** No release carries it. The only way to get it is
  `cargo install --path crates/taurus-cli` from a clone.
