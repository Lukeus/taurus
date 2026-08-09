# Taurus AI Shell

A desktop agent harness that runs against any model provider, starting with
local Ollama. Rust and Tauri v2 underneath, React on top, one codebase for
macOS, Windows, and Linux.

It reads and edits files in a workspace, runs commands, connects to MCP
servers, delegates to sub-agents — and writes down procedures it works out as
reusable **skills**, which you approve before they are kept.

## Quick start

```bash
ollama serve                # in another terminal
pnpm install
pnpm tauri dev
```

Pick a workspace from the header, pick a model, and start asking.

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
src-tauri/                  Windows, IPC, configuration — no agent logic
src/                        React UI
```

One rule holds the design together: **`src-tauri` contains no agent logic.**
The harness is drivable headlessly, which is why the agent loop can be tested
against a scripted provider and why the examples below work without a GUI.

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
and network access prompt with the exact call, and "allow always" persists to
`<workspace>/.taurus/permissions.json`. Shell approvals are keyed by the
leading command word, so approving `git` does not also approve `rm`.

Every path argument is canonicalized and checked against the workspace root,
which closes `../` traversal and symlink escapes alike.

## Configuration

| File | What it holds |
| --- | --- |
| `~/.taurus/providers.json` | Backends. API keys are referenced by env-var name, never stored. |
| `~/.taurus/mcp.json` | MCP servers, in the same format Claude Desktop uses. |
| `~/.taurus/settings.json` | Last workspace/model, skill-synthesis toggle. |
| `~/.taurus/skills/` | Skills available in every workspace. |
| `<workspace>/.taurus/skills/` | Skills that travel with the project. |
| `<workspace>/.taurus/permissions.json` | Persisted "allow always" decisions. |

## Development

```bash
cargo test --workspace     # 200 tests
pnpm test                  # transcript reducer
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

## Known gaps

- **MCP over HTTP is not wired up.** stdio servers work; the HTTP transport in
  `rmcp` pins a `reqwest` major version that conflicts with the one the
  provider adapters use. An HTTP entry in `mcp.json` reports this rather than
  failing silently.
- **`run_command` has no PTY.** Commands run non-interactively with stdin
  closed, which is right for an agent but means programs that check `isatty`
  behave as though piped, and interactive prompts hit the timeout instead of
  hanging forever.
- **Sessions are in-memory.** Config, skills, and permissions persist;
  conversations do not survive a restart.
- **Sub-agent progress is summarized, not streamed.** The parent's transcript
  shows one delegation card plus a tool-usage summary rather than the child's
  live output.
