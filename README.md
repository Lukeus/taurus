# Taurus AI Shell

An agent harness that runs against any model provider, starting with local
Ollama. Rust underneath, with two frontends over one shared core: a Tauri v2
desktop app and a `taurus` CLI. macOS, Windows, and Linux from one codebase.

It reads and edits files in a workspace, runs commands, connects to MCP
servers, delegates to sub-agents — and writes down procedures it works out as
reusable **skills**, which you approve before they are kept.

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
leading command word, so approving `git` does not also approve `rm`.

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
its **History** drawer switches between them.

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
| `providers.json` | Backends. API keys are referenced by env-var name, never stored. | Overrides and additions for this project. |
| `mcp.json` | MCP servers over stdio or HTTP, in the same format Claude Desktop uses. Header values and URLs may name env vars. | Extra servers, or `{"disabled": true}` to switch an inherited one off. |
| `settings.json` | Last workspace, skill-synthesis toggle, fallback model. | The provider and model this project was last worked in. |
| `skills/` | Skills available in every workspace. | Skills that travel with the project. |
| `permissions.json` | "Always everywhere" decisions. | "Always here" decisions. |
| `sessions/` | Transcripts, in a directory per workspace. | — |

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

`kind`, `api_key_env`, and the capability overrides are inherited from the
global entry with the same `id`. An entry whose `id` is new to this layer is
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
cargo test --workspace     # 276 tests
pnpm test                  # transcript reducer, replay, settings
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# TypeScript types are generated from Rust; regenerate after changing a payload:
TS_RS_EXPORT_DIR="$PWD/src/bindings" cargo test --workspace export_bindings
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
```

The CLI doubles as a live check on the whole stack:

```bash
taurus tools                    # what the agent can reach
taurus skills check             # non-zero exit if a skill is broken or degraded
taurus mcp                      # non-zero exit if a server failed to connect
```

`skills check` and `mcp` are meant for CI on a repository that ships its own
`.taurus/skills` directory.

## Known gaps

- **`run_command` has no PTY.** Commands run non-interactively with stdin
  closed, which is right for an agent but means programs that check `isatty`
  behave as though piped, and interactive prompts hit the timeout instead of
  hanging forever.
- **Sub-agent progress is summarized, not streamed.** The parent's transcript
  shows one delegation card plus a tool-usage summary rather than the child's
  live output.

## License

MIT. See [LICENSE](LICENSE).
