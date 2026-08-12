# Taurus AI Shell

An agent harness that runs against any model provider, starting with local
Ollama. Rust underneath, with two frontends over one shared core: a Tauri v2
desktop app and a `taurus` CLI. macOS, Windows, and Linux from one codebase.

It reads and edits files in a workspace, runs commands, searches the web,
connects to MCP servers, delegates to sub-agents — and writes down procedures it
works out as reusable **skills**, which you approve before they are kept. Every
file it edits is recorded first, so any turn can be **rewound**.

## Quick start

```bash
ollama serve                # in another terminal
```

Desktop app:

```bash
pnpm install
pnpm tauri dev
```

Or the CLI:

```bash
cargo install --path crates/taurus-cli

taurus repl                                     # interactive
taurus run "summarize the modules in src/"      # one-shot
taurus run --json "count the rust files" | jq   # for scripts
taurus rewind --to last                         # undo what the last turn wrote
```

Both share `~/.taurus` — same providers, same skills, same permission
allowlist. Approve a tool once in the app and your scripted runs inherit it.

## How it is put together

```
crates/
  taurus-provider/          Provider trait + normalized message/stream types
  taurus-provider-ollama/   Ollama adapter (NDJSON, per-model capabilities)
  taurus-provider-openai/   OpenAI-compatible adapter (SSE, vLLM/LM Studio/…)
  taurus-tools/             Tool registry, built-in tools, permission gate
  taurus-skills/            Skill discovery, execution, and authoring
  taurus-mcp/               MCP client
  taurus-web/               Web search and page fetching
  taurus-core/              Session state, the agent loop, sub-agents
  taurus-host/              Config, system prompt, registry assembly
  taurus-cli/               The `taurus` binary
src-tauri/                  Windows and IPC — no agent logic
src/                        React UI
```

One rule holds the design together: **a frontend contains no agent logic.**
Everything a session *is* — config files, system prompt, tool registry, skill
library — lives in `taurus-host`, and `Host::build_agent` is the single place
they come together. The desktop app and the CLI differ only in how they talk to
a person: how a permission prompt is asked, and where a skill proposal goes.
Both are traits.

That is also why the agent loop can be tested against a scripted provider, and
why the examples below run without a GUI.

### Provider-agnostic, concretely

The normalized types use Anthropic-style content blocks rather than OpenAI's
`tool_calls` shape, because blocks are the superset — a single assistant turn
with interleaved text, reasoning, and several tool calls survives the round
trip; the other direction does not.

Two things prove the abstraction rather than assert it:

- **The OpenAI adapter required no change to `taurus-core`**, despite a
  different transport (SSE vs NDJSON) and a different tool-call encoding
  (arguments as a string assembled across frames vs a whole object).
- **Models without tool support still call tools.** `gemma3` accepts no `tools`
  parameter at all. The harness detects that from Ollama's capability probe and
  switches to prompted tool calling, parsing `<tool_call>` blocks out of the
  text stream into the exact same events a native adapter emits. `taurus-core`
  cannot tell which path a turn took.

### Skills

A skill is a `SKILL.md` with YAML frontmatter plus optional bundled scripts,
discovered from `~/.taurus/skills` and `<workspace>/.taurus/skills`.

Only one line per skill — its `when_to_use` — enters the system prompt. The
procedure itself loads on demand via the `load_skill` tool. That is what makes
a fifty-skill library affordable on a model with an 8k context window.

The agent proposes new skills through `propose_skill`. Every proposal is
validated (kebab-case name, non-empty trigger under 200 characters, no
near-duplicate of an existing skill, no destructive script patterns) before it
reaches a review card, and nothing touches disk until you approve it. Approving
reloads the catalog, so a skill is usable in the session that wrote it.

Scripts declare a logical interpreter (`python3`, `node`, `bash`, …) which is
resolved per platform at load time. When it cannot be found, the skill is
marked degraded and the model is told to follow the written steps instead — a
Python-dependent skill does not hard-fail a Windows machine.

### Permissions

Read-only tools inside the workspace run unattended. Writes, command execution,
and network access prompt with the exact call. Shell approvals are keyed by the
leading command word, so approving `git` does not also approve `rm`. A call that
names a URL is keyed the same way by that URL's host: approving `fetch_url` for
`docs.rs` is a decision about a site, not a standing grant to reach anywhere.

An "allow always" decision persists into one of two layers, and both are
consulted:

| | Persists to | Asked again in a new workspace |
| --- | --- | --- |
| **Always here** | `<workspace>/.taurus/permissions.json` | Yes |
| **Always everywhere** | `~/.taurus/permissions.json` | No |

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

### Sessions

Every conversation is written as it happens, so closing the app or the terminal
does not end it:

```bash
taurus sessions                       # this workspace's, newest first
taurus repl --resume                  # pick up the most recent one
taurus run --resume <ID> "and now…"   # continue a named one
```

The desktop app reopens its last conversation for the workspace on launch, and
its left rail lists the rest — today's, then everything earlier — so switching
between them is one click rather than a drawer.

Transcripts live in `~/.taurus/sessions/<workspace>/<id>.jsonl`, in the global
config home rather than in the project. They hold file contents, command
output, and MCP responses; kept inside the workspace they would be committed by
accident. Keying the directory by workspace gives back the only thing that
location cost — "show me this project's sessions" — without putting any of it in
the repository.

The file is append-only, one JSON object per line: a header, then each message
as it is produced. Nothing is rewritten, so a crash costs the turn in flight
rather than the conversation, and a half-written final line is dropped on load
instead of poisoning the file. There is no index — everything a listing shows is
in each transcript's own opening lines, and an index is a second copy of the
truth that can disagree with it.

### Rewinding a turn

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

**`run_command` is not covered.** A shell command's reach cannot be known
before it runs, and the only honest options were to snapshot the whole
workspace before every command or to say plainly what is not included. Coverage
is exactly what a tool declares it will touch, which today means `write_file`
and `edit_file`. A file that was not text when it was recorded is reported as
`skipped` rather than silently left as the model made it, and `taurus rewind`
exits non-zero when anything could not be put back.

### The context window

A local 8k model runs out of room in a way a hosted 200k one does not, so the
budget is managed rather than hoped for. Three things do the work.

**Reads come back a window at a time.** `read_file` returns 2000 lines by
default and takes `offset` and `limit` for the rest. Line numbers stay absolute,
so a number from a windowed read still means what it says, and a partial answer
always says it is partial — a window that does not announce itself is
indistinguishable from a short file, and a model that thinks it read the whole
thing will act on what is missing.

**Old tool output shrinks before anything is summarized.** Tool results are most
of what a working session holds, and every byte is re-sent on each iteration of
the turn. When history crosses the compaction threshold, two cheap rules run
first, with no model call involved: a result whose call was later repeated with
the same input is dropped down to a pointer at the newer one, and results older
than the verbatim tail keep their first few lines and say what went. Only if
that does not get under budget is the older half summarized. The block itself
always stays either way — replacing its text keeps every tool call paired with a
result, which is the thing providers actually validate.

**Nothing is advertised that the prompt cannot explain.** Every tool's schema
goes out with every request — not once per session, once per iteration of every
turn — so it is the one part of the prompt that is pure overhead. Three things
keep it down. Schemas are slimmed on the way out: `$schema`, the Rust struct
name `schemars` leaves in `title`, `"default": null`, and integer-width formats
are dropped, which is about a quarter of the built-in schema bytes and applies
to MCP servers' schemas too. `propose_skill` is only registered when skill
synthesis is on, matching the prompt section that explains it — it is the
largest schema here, and offering it while saying nothing about it was paying
for a tool the model had no reason to call. And anything a project does not want
can be named in `settings.json`:

```json
{ "disabled_tools": ["fetch_url", "mcp__some-server__rarely_used"] }
```

A disabled tool is not registered at all, so skills and sub-agents cannot reach
it either — a tool hidden from the model but still callable would be a
permission gap wearing a token-saving costume. A name matching nothing is
reported rather than ignored, because a typo otherwise looks exactly like a tool
that is quietly still on.

**Where it went is a question you can ask.**

```bash
taurus usage            # this workspace's most recent session, by tool
taurus usage --all      # every session in this workspace
```

```
Turns              1
Messages           16
Billed by provider 20,440 in / 496 out
Transcript holds   ~1,407 tokens

Tool                    calls    ~tokens   share
read_file                   7      1,144    100%

Sent again with every request  ~1,982 tokens
  system prompt                 430
  10 tool schemas             1,552

Heaviest tool schemas
  propose_skill                 456
  read_file                     166
  run_command                   160
  edit_file                     156
  run_skill_script              151
  5 more                        463
```

The gap between the first two figures is the point: a transcript holding 1,407
tokens billed 20,440, and the bottom half is where the difference went — ~1,982
tokens of fixed overhead on each of seven requests. Per-tool numbers are
estimates, since a provider reports one total per request and never says which
part of the prompt was whose, but they use the same arithmetic that drives
compaction, so the report and the trigger cannot disagree. They are read back
out of the transcript rather than tracked beside it, for the reason the
transcript format already gives: a second copy of the truth can disagree with
it.

### Output formatting

Models answer in markdown, so both frontends render it.

The app parses markdown progressively as tokens arrive, tolerating the
half-finished constructs that streaming produces — an unclosed `**`, a code
fence with no terminator yet. Raw HTML is never rendered: model output is not
trusted markup, so `rehype-raw` is deliberately absent and any tags arrive
escaped as visible text. Links open in your browser rather than navigating the
webview away from the app.

The CLI applies ANSI attributes line by line: headings bold, `•` for bullets,
colored inline code, dimmed fenced blocks. With color off — piped, redirected,
or `NO_COLOR` — every line passes through byte for byte, so `taurus run >
out.md` still produces valid markdown.

The trade-off is that CLI prose appears a line at a time rather than a token at
a time, since a line has to be complete before it can be styled. The
alternative — redrawing the current line with cursor escapes — corrupts output
as soon as a line wraps.

## Configuration

The desktop app's **Settings** drawer edits providers, revokes permission
rules, and toggles skill synthesis. Everything it writes is a plain file under
`~/.taurus` that the CLI reads too, so the UI and a text editor are
interchangeable.

Every config file exists in two layers: the global `~/.taurus` and the
workspace's own `.taurus`. The workspace layer is read second and wins, the
same precedence skills use.

Settings edits the **global** layer, and says so when the current workspace
overrides one of the values on screen. That direction is deliberate: an editor
that saved the merged view back would write one project's overrides into the
file every other project reads.

| File | Global | Workspace |
| --- | --- | --- |
| `providers.json` | Backends, including the header a key is sent in. Never the key itself — that lives in the OS keychain or an env var. | Overrides and additions for this project. |
| `mcp.json` | MCP servers over stdio or HTTP, in the same format Claude Desktop uses. Header values and URLs may name env vars. Skills › **Edit mcp.json** opens it. | Extra servers, or `{"disabled": true}` to switch an inherited one off. |
| `search.json` | Web search backends and which one is active. Never the key itself — that lives in the OS keychain or an env var, as with providers. | A different backend for this project, or field overrides on an inherited one. |
| `settings.json` | Last workspace, skill-synthesis toggle, theme, fallback model. | The provider and model this project was last worked in. |
| `skills/` | Skills available in every workspace. | Skills that travel with the project. |
| `permissions.json` | "Always everywhere" decisions. | "Always here" decisions. |
| `sessions/` | Transcripts, in a directory per workspace. | — |
| `checkpoints/` | Pre-images of changed files, keyed by workspace like sessions and for the same reason. | — |

### API keys

A key never goes in a config file. Type it into Settings and it goes to the OS
credential store — Keychain on macOS, Credential Manager on Windows, the Secret
Service on Linux — under the service `taurus` and the provider's id. Web-search
backends keep their keys in the same store, under `search:<id>`, since both ids
are yours to choose and a backend and a provider may well share a name. Or from
a terminal:

```bash
taurus key set openai        # prompts, input not echoed
taurus key set openai < key.txt
pass show openai | taurus key set openai
taurus key set brave --search   # the same, for a web-search backend
taurus key status            # where every key comes from, both kinds
taurus key clear openai
```

The key is read from stdin, never from an argument: a key on the command line
is visible to every process on the machine through `ps` and lands in the shell
history of whoever typed it.

**An environment variable wins over a stored key.** Exporting one is an
explicit act, usually in CI or a container where no keychain exists at all, and
a stored key silently beating it would make headless runs unpredictable. So
`api_key_env` remains what it was, and is now optional — name a variable and it
takes precedence, leave it unset and the stored key is used. `taurus key
status` and the Settings field both say which one is in effect, because
"I stored a key and it isn't being used" is otherwise a 401 that explains
nothing:

```
$ taurus key status
ollama               none
openai               $OPENAI_API_KEY  (a stored key is being overridden)
azure                keychain
```

Settings never displays a stored key, only where the key comes from. The field
is a place to type a new one, not to review the old one — a secret handed to
the webview lives in JavaScript memory and in whatever the DOM does with it,
and nothing on that screen needs the value.

Two things worth knowing. On macOS the keychain grants access per binary, so
the first time the `taurus` CLI reads a key stored by the desktop app (or the
reverse) the OS asks you to allow it; "Always Allow" makes it once. And on
Linux the Secret Service is a running D-Bus service, not a file — on a headless
box there may be none, in which case storing fails, `taurus key status` says
so, and environment variables are the whole story.

### MCP servers

`mcp.json` takes stdio and streamable-HTTP servers in the format Claude Desktop
and Claude Code use, so an existing `mcpServers` block pastes in unchanged:

```jsonc
{
  "mcpServers": {
    "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"] },
    "remote": {
      "url": "https://mcp.example.com/mcp",
      "headers": { "Authorization": "Bearer ${EXAMPLE_MCP_TOKEN}" }
    }
  }
}
```

`${VAR}` in a header value or a URL is read from the environment. That matters
because a remote server almost always needs a credential, and the workspace
layer of `mcp.json` is meant to be hand-written and version-controlled — a
literal token there is a token in the repository. It is the same bargain
`providers.json` already makes for API keys. A variable that is not set fails
the server with its own name in the message, rather than sending an empty
`Authorization` header and producing a 401 that looks like a bad token instead
of a missing one. A literal value still passes through untouched.

### Web search

Two tools, `web_search` and `fetch_url`, and **neither exists until you turn
one on.** Searching means sending your prompt to a third party, which is not
something to start doing because a program was installed.

In the app, that is **Settings › Search**: pick a backend, paste its key, done.
The key goes to the OS keychain, the same place provider keys go, and the tools
are registered the moment the backend resolves — no restart.

Everything it writes is `~/.taurus/search.json`, which the CLI reads too and
which a first run leaves behind with every backend spelled out and none of them
selected:

```jsonc
{
  "backend": "brave",           // ← unset means off
  "backends": {
    "brave":   { "kind": "brave",   "api_key_env": "BRAVE_API_KEY" },
    "tavily":  { "kind": "tavily",  "api_key_env": "TAVILY_API_KEY" },
    "searxng": { "kind": "searxng", "base_url": "http://localhost:8888" }
  }
}
```

`api_key_env` stays supported and **wins over a stored key** when both are set,
which is what makes CI and keychain-less machines work. Same precedence as
providers, because both resolve through the same code.

| `kind` | Needs | Notes |
| --- | --- | --- |
| `brave` | An API key | Free tier covers ordinary use. |
| `tavily` | An API key | Built for agents; returns page extracts rather than one-line snippets. |
| `searxng` | A `base_url` | No key and no account. Your instance has to enable the `json` format in its `settings.yml`, or it answers API requests with a 403. |

Backends layer field by field like providers, so a workspace can retarget one
setting — `{"backends": {"brave": {"base_url": "http://proxy.internal"}}}` — or
switch which one is active with a bare `{"backend": "searxng"}`, without
restating the rest. `base_url` may name an environment variable the same way
`mcp.json` does.

Both tools carry the `network` effect, so both prompt with what they would
send: `web_search` shows the query in full, and `fetch_url` shows the whole URL
rather than an abbreviation of it — which host a request goes to is the
decision being approved, and a shortened URL is exactly what hides it.

Redirects are followed only while the host stays the same. One approval is
worth exactly one host, and the default policy would spend it somewhere else:
approve a link shortener and the hop lands wherever it points, including on
your own network. A redirect that crosses hosts stops and reports its target,
which the model can then ask for on its own terms.

**`fetch_url` will not reach your own machine or network.** The host is
resolved and every address it answers with has to be public, so
`http://127.0.0.1:8080/admin` and `http://169.254.169.254/` — the cloud
metadata endpoint, which answers unauthenticated and with credentials — are
refused. The URL there is chosen by a model that just read a web page, which
is what makes it different from a search backend or an MCP server: those are
addresses you wrote down, and they are never subject to this. If you want the
model reading a docs server you run locally, that is a deliberate act:

```jsonc
{ "allow_private_hosts": true }
```

Deliberately file-only, and not a checkbox in Settings. It is the one setting
here where the easy version of the mistake is expensive, and a config file is
a good place to make someone think for a moment.

The tools are registered together or not at all. A `fetch_url` with no way to
find a URL is only usable on links you paste, and search without fetch leaves
the model holding snippets it cannot follow. If the selected backend cannot run
— no key saved and none in the environment, a SearXNG entry with no URL —
nothing is registered and the reason is reported in Settings › Search, rather
than the model spending a turn discovering it has no credential. Picking a
backend and it still not running is a state the tab names explicitly, since a
selection alone is not the same as a working one.

### Azure OpenAI, and gateways in front of it

Azure is an OpenAI-compatible backend that disagrees about one thing: where the
key goes. OpenAI and everything imitating it read `Authorization: Bearer`;
Azure OpenAI reads `api-key`, and an Azure API Management gateway reads
`Ocp-Apim-Subscription-Key`. Both are bare — a `Bearer ` in front of the value
produces a 401 that looks exactly like a wrong key.

`api_key_header` names the header. The key is sent raw in it, with no scheme
prefix:

```jsonc
{
  "id": "apim",
  "kind": "open_ai_compatible",
  "base_url": "https://my-gateway.azure-api.net",
  "api_prefix": "/openai/v1",
  "api_key_env": "APIM_SUBSCRIPTION_KEY",
  "api_key_header": "Ocp-Apim-Subscription-Key",
  "models": ["gpt-4o", "gpt-4o-mini", "o3"],
  "default_model": "gpt-4o",
  "context_length": 128000
}
```

Leaving it unset keeps bearer auth, so nothing else changes. There is
deliberately no separate setting for the scheme: naming `Authorization` sends
the key bare in that header, which is the only other shape a gateway asks for.
The value is marked sensitive, so a subscription key cannot reach a debug log
through the header map.

Two other fields matter more here than elsewhere:

- **`api_prefix`.** Azure's OpenAI-shaped surface lives under `/openai/v1`,
  where the model goes in the request body. Its older data plane puts the
  deployment name in the path and requires an `api-version` query parameter;
  Taurus cannot express either, so point it at the `/openai/v1` route, or at an
  APIM route whose policy supplies them.
- **`models`.** A gateway need not expose `/v1/models`, and plenty of the ones
  that do answer with an inventory rather than an entitlement — every model the
  vendor sells, including the ones this key cannot call. Naming models here
  replaces that listing outright: what is listed is what the picker offers, and
  no request is made to find out. Leave it out and Taurus asks, which is right
  for Ollama and for any endpoint that answers usefully.

  An entry is either a bare id or an object, so the common case stays one word:

  ```jsonc
  "models": [
    "gpt-4o",
    { "id": "llama-3.1-8b", "context_length": 8192, "native_tools": false }
  ]
  ```

  The overrides matter because an OpenAI-compatible endpoint reports no
  capabilities at all, and one gateway commonly fronts models that share
  neither a context window nor tool support — told the provider-wide 128000
  above, an 8k model compacts tens of thousands of tokens too late. Anything
  left unset inherits the provider's own value, so a bare id means exactly what
  it did before these existed.

  A workspace layer *replaces* this list rather than adding to it. Appending
  could not express dropping a model, and a workspace that names models is
  saying which ones it wants.

- **`default_model`.** Which of them a new conversation starts on. Optional —
  the first model is used otherwise. It also still works alone, without
  `models`, which is all a single-model gateway ever needed. With neither, and
  no listing, the error says so instead of reporting an unreachable backend.

### Intel hardware, and other backends

Taurus never touches an accelerator. Every provider crate is JSON over HTTP —
there is no inference code and no device selection in this repository, so which
of your CPU, GPU, or NPU runs a model is decided entirely by the server you
point it at.

That matters because **stock Ollama on Intel runs on the CPU.** Getting Intel
acceleration means installing a different server, not configuring Taurus:

| Server | Intel target | Taurus config |
| --- | --- | --- |
| [OpenVINO Model Server](https://docs.openvino.ai/2025/model-server/ovms_what_is_openvino_model_server.html) | CPU, Arc GPU, Core Ultra NPU | `open_ai_compatible` |
| [IPEX-LLM's Ollama build](https://github.com/intel/ipex-llm) | Arc and integrated GPUs | `ollama`, unchanged |
| llama.cpp built with the SYCL backend | Arc and integrated GPUs | `open_ai_compatible` |

#### OpenVINO Model Server

Serve a model, choosing the device with `--target_device`:

```bash
docker run --rm -p 8000:8000 \
  -v $(pwd)/models:/models openvino/model_server:latest \
  --model_path /models/Qwen3-8B-int4-ov --model_name qwen3 \
  --task text_generation --target_device NPU --rest_port 8000
```

Models must be in OpenVINO IR format — export with
`optimum-cli export openvino`, or pull a pre-converted one from the `OpenVINO`
org on HuggingFace.

```jsonc
{
  "id": "openvino",
  "kind": "open_ai_compatible",
  "base_url": "http://localhost:8000",
  "context_length": 8192,
  "native_tools": true
}
```

Three settings decide whether this works, and getting them wrong fails in ways
that do not look like configuration errors:

- **`api_prefix`.** OVMS served the OpenAI routes under `/v3` until 2026.3
  added `/v1` as an alias. On anything earlier, add `"api_prefix": "/v3"` — the
  symptom otherwise is a flat `404` from a server that is running fine.
- **`context_length`.** OVMS caps prompts at 8k tokens on NPU. This value is
  what drives compaction, and it cannot be probed over the OpenAI API, so it
  defaults to 128000 — roughly sixteen times too high for an NPU. History would
  never be compacted before the server began rejecting requests, and the
  failure surfaces as a provider error rather than as "this conversation got
  too long".
- **`native_tools`.** Tool calling in OVMS needs a per-model *tool parser* —
  Qwen3, Hermes3, Llama3, Mistral-7B-v0.3, phi-4-mini and others have one.
  Serving a model without one and leaving `native_tools` at its default of true
  gives a model that narrates tool calls it never actually makes. Set it to
  `false` and Taurus switches to prompted tool calling, the same fallback it
  uses for `gemma3` on Ollama.

`api_prefix` is not OpenVINO-specific: any server behind a reverse proxy that
mounts the API elsewhere needs it, and `""` puts the routes directly on the
base URL.

Layering is per key, not per file, so an override states only what it changes:

```jsonc
// <workspace>/.taurus/providers.json — point one provider at a different box
[{ "id": "ollama", "base_url": "http://gpu-box:11434" }]
```

`kind`, `api_key_env`, `api_key_header`, and the capability overrides are
inherited from the global entry with the same `id`. An entry whose `id` is new
to this layer is
added instead, and then needs a `kind` and a `base_url` of its own. Anything
unresolvable — a malformed layer, an override with no `kind`, an MCP toggle
naming a server nothing defines — is reported as a startup problem rather than
silently dropped, and the other layer still loads.

Because the workspace layer belongs to a directory you can change at any time,
all of it is re-resolved on every workspace switch, not just at startup.

Two values are deliberately not layered. `last_workspace` is written globally
only — a pointer to a workspace, stored inside the workspace it names, could
only ever point at itself. Edits from the app's provider settings also write
globally: the workspace layer is meant to be hand-written and version-controlled,
and round-tripping the merged view into it would bake every inherited value
into the project file.

## Development

```bash
cargo test --workspace     # 322 tests
pnpm test                  # transcript reducer, replay, settings, rewind
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# TypeScript types are generated from Rust; regenerate after changing a payload.
# `src/bindings` is not committed, so this is also the first thing a fresh clone
# needs — without it `pnpm build` cannot find a single frontend type.
pnpm bindings
```

### Live checks

These run against a real Ollama server and are the fastest way to confirm a
change did not break the parts that unit tests cannot reach.

```bash
# One provider, one turn, one tool call.
cargo run -p taurus-provider-ollama --example smoke -- qwen3.6:27b
cargo run -p taurus-provider-ollama --example smoke -- gemma3      # prompted fallback

# The OpenAI adapter, against Ollama's own /v1 endpoint.
cargo run -p taurus-provider-openai --example smoke -- llama3.2:latest

# The whole harness: read files, write a file, report what happened.
cargo run -p taurus-core --example e2e -- qwen3.6:27b

# Skill authoring: propose, validate, save, rediscover.
cargo run -p taurus-skills --example synthesis -- qwen3.6:27b

# MCP: connect, list tools, call one through the registry.
cargo run -p taurus-mcp --example probe -- path/to/mcp.json

# Web: one real search, then fetch the first result it returns.
cargo run -p taurus-web --example probe -- ~/.taurus/search.json "rust async book"
```

The CLI doubles as a live check on the whole stack:

```bash
taurus tools                    # what the agent can reach
taurus skills check             # non-zero exit if a skill is broken or degraded
taurus mcp                      # non-zero exit if a server failed to connect
taurus key status               # where each provider's API key comes from
```

`skills check` and `mcp` are meant for CI on a repository that ships its own
`.taurus/skills` directory.

## Known gaps

- **A rewind does not cover `run_command`.** Checkpoints record what a tool
  declares it will touch, and a shell command's reach is not knowable before it
  runs. See [Rewinding a turn](#rewinding-a-turn).
- **`run_command` has no PTY.** Commands run non-interactively with stdin
  closed, which is right for an agent but means programs that check `isatty`
  behave as though piped, and interactive prompts hit the timeout instead of
  hanging forever.
- **A sub-agent's answer is summarized, not streamed.** Its tool calls now
  appear under the delegation card as it makes them, so a long delegation looks
  alive rather than hung, but its reasoning and prose stay inside the child.
  That part is deliberate: the parent asked for a conclusion, and a second
  conversation inlined into the transcript is what delegation exists to avoid.
- **`fetch_url` reads the HTML it is served.** No JavaScript runs, so a page
  that renders its content client-side comes back near-empty. Closing this
  means shipping a browser engine, so it is a limit rather than a to-do.
- **`fetch_url` resolves a name twice.** Loopback and private-network addresses
  are refused — the host is resolved and every address it answers with must be
  public — but the connection resolves the name again, so a name that answers
  publicly during the check and privately a moment later would get through.
  Pinning the connection to the address that was checked is per-client in
  reqwest, not per-request. `"allow_private_hosts": true` in `search.json`
  turns the check off deliberately.

## License

MIT. See [LICENSE](LICENSE).
