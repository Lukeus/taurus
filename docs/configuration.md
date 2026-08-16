# Configuration

<sub>[← Taurus AI Shell](../README.md)</sub>

The desktop app's **Settings** drawer edits providers, revokes permission
rules, and toggles skill and sub-agent synthesis. Everything it writes is a plain file under
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
| `settings.json` | Last workspace, the two synthesis toggles, theme, fallback model. | The provider and model this project was last worked in. |
| `skills/` | Skills available in every workspace. | Skills that travel with the project. |
| `permissions.json` | "Always everywhere" decisions. | "Always here" decisions. |
| `sessions/` | Transcripts, in a directory per workspace. | — |
| `checkpoints/` | Pre-images of changed files, keyed by workspace like sessions and for the same reason. | — |

## API keys

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

## MCP servers

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

**The agent can draft an entry but never install one.** `draft_mcp_server`
takes a name and a command line and hands back a block to paste, the file it
belongs in, and what has to be filled in first — and that is all it does. It
writes nothing and starts nothing.

That asymmetry with skills and sub-agents is deliberate. Both of those are
reviewable: the artifact you approve is the text that will run. An MCP entry is
a pointer to code nobody in the loop has seen — the reviewable part of `npx -y
@scope/package` is a package name — and the program it names runs at every
launch, before any tool call, outside the permission engine. A review card
there would be asking for a decision with the information missing. So the model
does the part it is good at, which is knowing what the server is called and
which arguments it takes, and installing stays something you do in your editor
having read it.

Secrets are never carried through the draft. `env` and `headers` take variable
and header *names*; the block comes back with `<replace-me>` where each value
goes, and the model is told to explain what each one is rather than guess at it.
A key the model typed would live in the transcript, and in every copy of it, for
as long as the conversation is kept. The block is rendered through the same type
the loader reads, so what comes back is what will parse.

## Web search

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
refused. That resolution happens inside the HTTP client `fetch_url` uses, so
the addresses the connection is given are the addresses that were checked;
there is no second lookup for a name to answer differently. The URL there is
chosen by a model that just read a web page, which
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

## Anthropic and Google Gemini

Both are their own `kind` rather than a `base_url` pointed at a different host,
because neither is OpenAI-shaped. Anthropic reads the key from `x-api-key`, puts
the system prompt in a top-level field, and sends tool input as an object;
Gemini calls the assistant `model`, gives tool calls no ids at all, and takes an
OpenAPI subset where the others take JSON Schema.

```jsonc
[
  { "id": "anthropic", "kind": "anthropic", "base_url": "https://api.anthropic.com" },
  { "id": "gemini", "kind": "gemini", "base_url": "https://generativelanguage.googleapis.com" }
]
```

That is the whole configuration. Keys go in the OS keychain as usual — `taurus
key set anthropic` — or in a variable named by `api_key_env`.

**Neither needs a `context_length`,** and neither should be given one except as
a fallback. Anthropic reports a window and a capability tree per model, so
Taurus asks; Gemini reports a window in its model listing. Each answer is
remembered per model for the life of the provider, because compaction asks the
question once per iteration of the agent loop and the number cannot change while
a turn runs — probing each time would put a round trip in front of every model
call. A configured value
that disagrees with the model is how a conversation compacts at the wrong
moment, so the field is offered in Settings as "only used if the backend will
not report its own window" and left empty by default.

**Prompt caching is on by default on Anthropic.** The system prompt and tool
schemas are exactly the fixed overhead [`taurus usage`](working-with-it.md#the-context-window)
exists to report — re-sent on every iteration of every turn — and this is the
one backend here that will serve them back at about a tenth of the price. Two
breakpoints of the four allowed: one after the system prompt, which also covers
the tools rendered before it, and one on the newest turn, so the cached prefix
grows with the conversation rather than resetting each iteration. Cached tokens
are counted into the input total, so a well-cached turn reports what the request
carried rather than only the part that missed.

**Thinking is left to the model by default.** Sending no `thinking` field is the
only setting valid on every model that API has served — the newer ones reason by
default and the older ones do not, and neither rejects a request that says
nothing. `"thinking": "adaptive"` or `"disabled"` overrides it, and the wrong
one is a 400 rather than a preference, which is why it is not guessed.

Reasoning blocks are replayed with the signature the provider issued them under.
That is not a nicety: a turn that reasoned and then called a tool is only legal
on the next request if its thinking comes back signed and unedited, so a
signature that did not survive the stream is a rejected request one turn later.

**Gemini's schemas are sanitized on the way out.** It accepts an OpenAPI 3
subset and refuses a request outright on a keyword it does not know, with an
error naming the tool rather than the offending word — so `$schema`, `title`,
`additionalProperties`, and the integer-width `format`s that `schemars` emits
are stripped at every level of every tool schema. Its tool calls carry no ids,
so Taurus synthesizes them and resolves them back to names on the way out;
without that, two calls to the same tool in one turn would be indistinguishable
and so would their results.

## Azure OpenAI, and gateways in front of it

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

## Intel hardware, and other backends

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

### OpenVINO Model Server

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
