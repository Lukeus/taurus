# Development

<sub>[← Taurus AI Shell](../README.md)</sub>

```bash
cargo test --workspace     # 791 tests
pnpm test                  # transcript reducer, replay, settings, rewind, diffs
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# The README's screenshots. Needs Chrome or Chromium; nothing else does.
pnpm screenshots

# TypeScript types are generated from Rust; regenerate after changing a payload.
# `src/bindings` is not committed, so this is also the first thing a fresh clone
# needs — without it `pnpm build` cannot find a single frontend type.
pnpm bindings
```

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
at a picture — so the mark and the icon are now one file apart rather than two
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
is the whole of it.

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
# through the registry. Reports entries that would not parse, so a typo does not
# pass in silence now that it no longer takes its neighbours down with it.
cargo run -p taurus-mcp --example probe -- path/to/mcp.json

# Web: one real search, then fetch the first result it returns.
cargo run -p taurus-web --example probe -- ~/.taurus/search.json "rust async book"

# What a sweep costs on a real workspace, and that it stays quiet when nothing
# changed. Needs no provider. Run it on something large before touching the
# caps in `sweep.rs` — every command pays this twice.
cargo run -p taurus-tools --example sweep -- .

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
