# Configuration

<sub>[← Taurus AI Shell](../README.md)</sub>

The desktop app's **Settings** drawer edits providers, revokes permission
rules, and toggles skill and sub-agent synthesis. Everything it writes is a plain file under
`~/.taurus` that the CLI reads too, so the UI and a text editor are
interchangeable.

Every config file exists in two layers: the global `~/.taurus` and the
workspace's own `.taurus`. The workspace layer is read second and wins, the
same precedence skills use — **once you have said that workspace's config may
be read.** Until then only the global layer applies. See
[Trusting a workspace](#trusting-a-workspace).

Settings edits the **global** layer, and says so when the current workspace
overrides one of the values on screen. That direction is deliberate: an editor
that saved the merged view back would write one project's overrides into the
file every other project reads.

| File | Global | Workspace |
| --- | --- | --- |
| `providers.json` | Backends, including the header a key is sent in. Never the key itself — that lives in the OS keychain or an env var. | Overrides and additions for this project. |
| `mcp.json` | MCP servers over stdio or HTTP, in the same format Claude Desktop uses. Values may name env vars. The **MCP** panel reads and writes it; **Edit mcp.json** opens it. | Extra servers, or `{"disabled": true}` to switch an inherited one off. |
| `search.json` | Web search backends and which one is active. Never the key itself — that lives in the OS keychain or an env var, as with providers. | A different backend for this project, or field overrides on an inherited one. |
| `settings.json` | Last workspace, the two synthesis toggles, theme and theme id, fallback model, `max_iterations`. | The provider and model this project was last worked in, and a step limit for turns here. |
| `themes/` | Custom palettes, typefaces, wordmarks and corner radii. See [Themes](#themes). | Themes that travel with the project, so a repository can brand the app for everyone who opens it. |
| `skills/` | Skills available in every workspace. | Skills that travel with the project. |
| `permissions.json` | "Always everywhere" decisions. | "Always here" decisions. |
| `sessions/` | Transcripts, in a directory per workspace. | — |
| `checkpoints/` | Pre-images of changed files, keyed by workspace like sessions and for the same reason. | — |
| `hooks.json` | Programs run at fixed points in a turn. | Extra hooks, or `{"disabled": true}` to switch an inherited one off. |
| `trust.json` | Which workspaces' own config may be read. Global only — a repository that declared itself trusted would have declared nothing. | — |

## Trusting a workspace

The workspace layer is not passive data. `mcp.json` starts child processes,
`providers.json` names the endpoint your conversation is sent to, `search.json`
decides whether `fetch_url` may reach private hosts, `permissions.json` is a
standing grant, and a skill can carry a script. All of that travels in a
repository, and a repository is something you may have cloned a minute ago.

So there is one rule, and it goes in one direction: **an untrusted workspace
contributes no config at all.** Your own `~/.taurus` still applies in full, so
Taurus works normally in a fresh clone. What it does not do is take instructions
from the clone.

**Nothing is asked about a workspace that has no config of its own**, which is
most of them. The question only appears when the folder actually holds
something, and it says what:

```
This project has configuration Taurus is not reading.
  1 skill
  1 MCP server
      probe: npx -y some-package
  2 standing permission grants — tools this project would allow without asking
```

The MCP command lines are named rather than counted, because a command line is
the only part of that list you can actually judge, and it is also the part that
starts a process on your machine.

In the desktop app this is a banner above the composer, not a modal on open. The
decision is not urgent — nothing from the folder is loaded, so nothing is
waiting on an answer — and a modal you have to clear before starting work is how
a security prompt becomes a reflex. **Not now** dismisses it for that window and
records nothing.

In the terminal every command prints one line when a workspace has config going
unread, and `taurus trust` is where you answer:

```
taurus trust             # what this workspace holds, and whether it is read
taurus trust --allow     # read it, from now on
taurus trust --revoke    # stop reading it
taurus trust --list      # every workspace trusted so far
```

Answering takes effect immediately rather than on the next launch: trusting a
workspace loads its skills and agents and connects its servers there and then,
and revoking unloads them and shuts the servers down.

One consequence worth knowing: in an untrusted workspace the permission prompt
offers **Allow once** and **Deny** but not **Always here**. There is no
workspace layer to keep a standing decision in, and a button that promised
permanence it could not deliver would be worse than one that is absent.

## Hooks

A hook is a program Taurus runs at a fixed point in a turn. There is no API to
build against and nothing to compile — a command line in `hooks.json`, told
what is about to happen on stdin, answering with an exit code.

```json
{
  "hooks": {
    "no-force-push": {
      "on": "pre_tool_use",
      "command": "./scripts/no-force-push.sh",
      "matches": { "tools": ["run_command"], "commands": ["git"] }
    },
    "format-rust": {
      "on": "post_tool_use",
      "command": "cargo",
      "args": ["fmt"],
      "matches": { "paths": ["**/*.rs"] }
    }
  }
}
```

**A hook can refuse and cannot permit.** `pre_tool_use` runs *after* the
permission engine has already allowed a call, so a hook can stop something you
permitted and can never permit something you refused. Adding hooks to a machine
only ever shrinks what it will do. That is deliberate: the alternative is a
second permission system sitting beside the first and disagreeing with it, and
it is also what makes a project's hook file safe to honour at all once the
project is trusted.

| Event | When | Can it stop anything? |
| --- | --- | --- |
| `pre_tool_use` | Before a tool call, after permission | Yes |
| `post_tool_use` | After a tool call, pass or fail | No — the call has happened |
| `user_prompt_submit` | When you send a message | Yes |
| `stop` | When a turn ends | No |

`matches` narrows which calls a hook is about, and every field is optional —
leave it out entirely and the hook applies to everything on its event.
`commands` is keyed by the leading word of a command line, the same unit an
"always allow" decision uses, so `git` never matches `rm`. `paths` globs against
the workspace-relative paths a call names, read from each tool's own declaration
of what it touches, so one glob covers every tool that writes.

Hooks are run with **no shell**. `command` is a program and `args` are its
arguments; if you want a pipeline, put it in a script and name the script.

**What a hook is told** — one JSON object on stdin. Reading it is optional:

```json
{
  "event": "pre_tool_use",
  "workspace": "/Users/me/project",
  "session_id": "s-1a2b",
  "tool": "run_command",
  "input": {"command": "git push --force"},
  "paths": ["src/widget.rs"]
}
```

`TAURUS_HOOK_EVENT`, `TAURUS_WORKSPACE`, and `TAURUS_TOOL` are in the
environment too, for a hook that would rather not parse JSON. The working
directory is the workspace.

**What a hook says back** — its exit code, and nothing else. Not a JSON protocol
on stdout: a hook is usually three lines of shell, and a format it has to emit
*correctly* to be obeyed is one that will sometimes be emitted incorrectly and
silently ignored.

| Exit | Meaning |
| --- | --- |
| `0` | Fine. Anything on stdout reaches the model as a note. |
| `2` | Refused. stderr — or stdout — becomes the reason the model is given. |
| anything else | The hook did not work. |

**A hook that cannot run refuses.** A missing program, a crash, a timeout: on
`pre_tool_use` and `user_prompt_submit`, all of these deny. This is the
uncomfortable half and it is on purpose — a hook exists to make a decision, and
one that could not make a decision has not approved anything. A typo in
`hooks.json` blocking every call is loud, names the hook and the exit code, and
is fixed in seconds; a guard that silently stops guarding is not fixed at all,
because nobody knows. On `post_tool_use` and `stop` there is nothing left to
stop, so a failure there is reported and the turn continues.

`timeout_seconds` defaults to 30. A hook runs inside a turn, so a hook that
hangs is a turn that hangs — and a hook that reaches its limit is killed and
counted as a refusal, on the events that can still refuse. The limit covers the
whole of it, including the payload being handed to a hook that never reads its
stdin.

The kill reaches the whole tree, not just the program the hook names: a script
that calls a linter takes the linter with it. On Unix the hook runs in a process
group of its own and the group is signalled; on Windows the tree is ended with
`taskkill /T`, which walks down from the hook and so cannot reach a process
whose own parent died first — see [Known gaps](known-gaps.md).

Seeing what will run, and why something is not:

```
taurus hooks list      # every hook that will run, and what narrows it
taurus hooks check     # entries that would not load, with the field named
```

Hooks follow the trust gate like every other layered file: a workspace's own
`hooks.json` does nothing until that workspace is trusted. See
[Trusting a workspace](#trusting-a-workspace).

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
`api_key_env` is optional — name a variable and it takes precedence, leave it
unset and the stored key is used. `taurus key status` and the Settings field
both say which one is in effect, because "I stored a key and it isn't being
used" is otherwise a 401 that explains nothing:

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

The MCP panel's **Browse servers** carries the setup for the ones Taurus knows —
what the command is, which argument the directory goes in, which header holds
the token — so adding GitHub does not start with reading a README. Filling one
in produces an ordinary entry and hands it to the same form an entry typed by
hand goes through, so the command line is on screen before anything is written
and **Test** is the same Test.

![The catalogue, listing the servers Taurus knows the setup
for](screenshots/mcp-catalog.png)

Adding one writes into `mcp.json` and nothing else. Nothing is downloaded and no
installer runs — `npx` and `uvx` fetch the program at launch exactly as they do
for an entry you typed. The blast radius of pressing the button is a config
file.

The list ships with the application rather than being fetched, and every entry
is one somebody reviewed in a commit against its source, which each card links.
That is the whole difference between this and a registry search: the reviewable
artifact for `npx -y @scope/package` is a package name, which says nothing about
what the package does, and a search box handing those back would be asking for a
decision nobody in the loop can make. The cost is that it goes out of date
between releases — the panel shows when it was last checked — and that a server
not on it is added by hand, which is one extra step rather than a dead end. A
list that has gone stale cannot break a working setup: installing copies the
entry into `mcp.json` and the catalogue never looks at it again.

**Some of what you will search for is not there, and says why.** Postgres has
had no first-party server since the reference one was archived and deprecated
over a SQL-injection vulnerability, and nothing official replaced it. Entries
like that carry the reason instead of a button, because searching for the thing
you cannot have should return an explanation rather than nothing.

### Signing in

A hosted server — Linear, Google Drive, and most of what vendors now run — is
secured with OAuth rather than a token you can paste. Add the entry, then press
**Sign in** on its card: Taurus discovers the authorization server, registers
itself, and opens your browser. Approving there sends the browser back to a
loopback address Taurus is listening on for exactly one request, and the tokens
land in the OS keychain — the same place provider API keys go, and never in
`mcp.json`.

Nothing starts that flow on its own. A connection that opened a browser window
because a server answered 401 would be the application taking over the screen in
response to something you did not do, so a server that needs an account says so
and waits to be asked.

Every request carries a token minted for it, refreshed when it has expired, so a
window left open overnight goes on working without a tool call failing at the
first use after the token lapsed. **Sign out** forgets Taurus's copy; the grant
itself stays until you remove the application in the provider's own settings,
which is the only place it can be revoked.

Stdio servers have no sign-in and are not offered one — the MCP authorization
specification is explicit that a local program takes its credentials from the
environment, which is what `${VAR}` below is for.

**A credential goes in the global file by default.** Every catalogued entry that
wants one defaults to `~/.taurus/mcp.json`, which is not a file anybody commits.
Choosing this project instead writes it into `<workspace>/.taurus/mcp.json` — a
file inside a repository, one `git add .` from being published — so that
combination asks you to confirm rather than going through quietly. It is not
refused; there are good reasons to want it.

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

`${VAR}` is read from the environment wherever a value appears — a URL, a header
value, a stdio `command`, an argument, an `env` value. That matters because a
server almost always needs a credential, and the workspace layer of `mcp.json`
is meant to be hand-written and version-controlled — a literal token there is a
token in the repository. It is the same bargain `providers.json` already makes
for API keys. A variable that is not set fails the server with its own name in
the message, rather than sending an empty `Authorization` header and producing a
401 that looks like a bad token instead of a missing one. A literal value still
passes through untouched.

### The MCP panel

**MCP** in the rail lists every configured server across both layers, with what
each one is, whether it connected, and what it offers. Adding, editing,
enabling, and removing all write `mcp.json`, one entry at a time — everything
else in the file is copied through untouched, including keys this version does
not model. So a server added in the panel and one pasted in by hand are the same
thing, and neither route disturbs the other's work. **Edit mcp.json** is still
there for anything the form cannot express.

**Test** connects the entry in front of you, reports the tools it found, and
disconnects. It registers nothing and leaves any live connection alone, so an
edit can be checked before it is saved rather than after.

Each connected server carries what it costs: **~1.2k tokens of every request**,
and a total for the drawer beside the tool count in the header. That is the
number the switch next to it is for. A tool is paid for on every iteration of
every turn whether or not the model calls it, so four servers left on out of
habit are four servers the conversation starts owing — which on an 8k window is
the difference between a harness that works and one with no room left to think
in. It is the same figure the Context panel reports as the fixed half, narrowed
to one server, and it comes from the same arithmetic so the two cannot disagree.

A server that never connected shows no figure rather than a zero. Its tools are
not registered, so there is nothing to measure; what you can read here is what
enabling one cost you, not what enabling one would cost. A server that connected
and offers nothing shows a real `~0`, which is worth saying out loud — it is a
child process running for no benefit.

A stored value that is not a `${VAR}` reference is treated as a secret: the
panel is told the key is set and never given the value. Leaving that field alone
keeps what is on disk; typing over it replaces it.

Saving reconnects the MCP servers and nothing else — skills, providers, and the
index are untouched, which is the same rule agent edits already follow. The one
exception is the agent roster, and only when a save changed which tools the
servers offer: an agent can be scoped to an MCP tool, so adding the server it
needs has to make that agent usable, and deleting it has to stop the roster
claiming a tool that is gone.

### When a server will not start

The most common failure is not a wrong entry. An app launched from the Dock or
Finder inherits the launcher's environment, and on macOS that PATH is
`/usr/bin:/bin:/usr/sbin:/sbin` — no Homebrew, no nvm, no pyenv, no
`~/.local/bin`. `npx` and `uvx` live in exactly those places, so a correct entry
for an installed program fails with "command not found".

Taurus asks your login shell for its PATH once at startup and merges what it
finds, which fixes this for most setups. `-l -i`, because nvm and pyenv install
themselves into `.zshrc` rather than `.zprofile`. The panel's **Program search
path** section shows the result: the directories being searched, which of them
the shell contributed, and — when a server's command is not among them — that it
could not be found. Set `TAURUS_SKIP_LOGIN_PATH=1` to skip the probe; a program
named by its full path never needed it.

Other things the panel will tell you rather than leave you to find:

- A server that will not parse is reported **by name, with the key that is
  wrong**, and its neighbours still load, rather than one typo discarding every
  server in the file.
- A server switched off is listed as `off` rather than disappearing.
- A server that never answers is given up on after 60 seconds, so one hung
  program cannot stall the reload the others are waiting for.
- A server that **stops** answering part way through a session is moved back to
  red with the reason, rather than going on being listed as connected while
  every call against it fails. Press **Reconnect** to start it again.

### What a call may take

Two limits sit around every tool call an MCP server serves, because a server is
a program nobody here reviewed running in the same window as everything else.

**Two minutes of silence.** A call that goes two minutes without a word is given
up on, and the model is told the server either hung or reports no progress. The
clock measures *silence*, not work: a server that sends progress notifications
restarts it every time, so a job that legitimately takes an hour and says so is
never cut off. There is no ceiling above that — how long your build takes is not
something this can usefully guess — and **Stop** ends a call at any point,
telling the server to stop rather than merely walking away from the answer.

**A share of the context.** A result too large for the window is cut to its head
and its tail, with a line saying how many bytes went and where they went: the
whole answer is written out to the same place long command output goes, so the
middle is a `read_file` away rather than lost. Pictures are never cut — half an
image is a broken image — and the share is the same one the shell tool takes.

### What the agent may and may not do

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
which arguments it takes, and installing stays something you do.

The panel does not change that. It is a form *you* fill in — the agent cannot
reach it, and drafting still writes nothing and starts nothing. What it changes
is where you do the installing: in a window that can tell you the entry is
malformed, that the program is not on the PATH, and whether the server actually
answers, rather than in a text editor that can tell you none of those.

Secrets are never carried through the draft. `env` and `headers` take variable
and header *names*; the block comes back with `<replace-me>` where each value
goes, and the model is told to explain what each one is rather than guess at it.
A key the model typed would live in the transcript, and in every copy of it, for
as long as the conversation is kept. The block is rendered through the same type
the loader reads, so what comes back is what will parse.

## Themes

The window ships two palettes and follows your system between them. A **theme**
is the third thing: whose colours, typefaces and wordmark those palettes wear.
It is a file in `~/.taurus/themes/`, and Settings › Appearance is an editor over
that file rather than the only way to write one.

![Settings, Appearance](screenshots/appearance.png)

The reason a theme is so small a file is that `src/styles.css` names its raw
values exactly once and speaks in *roles* everywhere else — a panel is
`--bg-raised`, a hairline is `--rule`, the lead accent is `--accent`. Fourteen
colours at the top move six thousand lines below them. So a theme supplies
fourteen colours, three typefaces, a wordmark and a corner radius, and nothing
else. There is no stylesheet, no selector and no length that is not one of those:
a theme that could restate a rule could break a layout in a way only its author
could reproduce.

### The file

Everything is optional. The common case is wanting a different accent, and it
costs four lines:

```json
{
  "name": "Midnight",
  "dark": { "accent": "#b48cff", "accent-hover": "#c9aaff" }
}
```

Anything left out falls through to the palette the app ships, which is also what
keeps a theme written today working after the app adds a token tomorrow. A fuller
one:

```json
{
  "name": "Acme",
  "dark": {
    "ink": "#07090d",
    "surface-1": "#10141c",
    "surface-2": "#182131",
    "surface-hover": "#141a26",
    "line": "#26314a",
    "text": "#eef2f6",
    "text-dim": "#9aa6bb",
    "text-faint": "#7c8b9c",
    "accent": "#b48cff",
    "accent-hover": "#c9aaff",
    "on-accent": "#07090d",
    "ok": "#a3ffb0",
    "warn": "#ffbb7c",
    "danger": "#ff9a9a"
  },
  "light": { "accent": "#6b3fd4", "on-accent": "#ffffff" },
  "fonts": { "display": "IBM Plex Sans", "body": "Inter", "mono": "JetBrains Mono" },
  "brand": { "wordmark": "acme", "logo": "acme.svg" },
  "shape": { "radius": 0.4, "gutter": 28, "rail-gutter": 18 }
}
```

| Key | What it is |
| --- | --- |
| `name` | What the picker calls it. Falls back to the file name. |
| `dark`, `light` | The fourteen colours, by the names in the table below. Hex only — `#rgb`, `#rrggbb` or `#rrggbbaa`. |
| `fonts` | `display`, `body`, `mono`. A family name, not a stack: the fallbacks after it stay the app's, so naming a font you do not have degrades rather than breaks. It has to be **installed on the machine** — the window loads no remote stylesheets, so a theme cannot bring a typeface with it. |
| `brand.wordmark` | The word beside the mark. An empty string is a real answer and means a mark on its own; leaving the key out keeps `taurus`. |
| `brand.logo` | An SVG, PNG, JPEG or WebP up to 256KB. A bare name is read from the folder the theme file is in, so a logo committed beside it travels with it. |
| `shape.radius` | Multiplier on the corner-radius ladder, 0 to 3. `0` is square, `1` is as shipped. |
| `shape.gutter`, `shape.rail-gutter` | The two column insets, in px, up to 96. |

The colour names are the *jobs*, not the colours. The design system names its
accents after what they happen to be — cyan, peach, mint — which is fine for a
system with one palette and absurd in a file whose whole point is that the accent
might be violet.

| Name | Where it is |
| --- | --- |
| `ink` | The window itself, and the native ground behind the webview. |
| `surface-1` | A panel raised off it — the rail, a drawer, a card. |
| `surface-2` | A panel raised off that, and the active state of a row. |
| `surface-hover` | The step between, so a hover reads as on the way to a selection rather than as one. |
| `line` | The one hairline weight. |
| `text`, `text-dim`, `text-faint` | Three weights, brightest first. The faint one carries the 10px mono micro-labels. |
| `accent`, `accent-hover` | The lead colour. |
| `on-accent` | What stays legible on top of it — a filled button's label. |
| `ok`, `warn`, `danger` | The three signals. |

### Dark, light, or both

`dark` and `light` are separate palettes rather than one palette with a base,
because "follow the system" is a preference people keep and a theme that could
not honour it would be a theme that quietly takes it away. Fill in both and the
System/Light/Dark choice keeps working underneath your brand.

Fill in only one and you have said something — *this brand is dark* — so
selecting it pins the mode, and the three pills say why instead of sitting there
appearing to do nothing. A theme that only changes the typeface and the wordmark
names neither palette and is as good in daylight as at night.

### Contrast

The editor measures every pair the app actually puts on screen — body text on
each of the three surfaces, the faint labels, the accent, the label on a filled
button, the three signals — and names the ones that come out below 4.5:1, in
words that say where each is rather than which two tokens it is between.

It warns rather than refuses. The floors are WCAG's and they are the right
default, but this is your machine and your screen, and a checker that would not
let you save a 4.2:1 would be enforcing a taste. What it will not do is let it
happen silently, which is the state a branding feature arrives in if nobody
builds this.

### Where a theme can live

Both config layers, like everything else here. `~/.taurus/themes/` is yours;
`.taurus/themes/` inside a workspace travels with the repository, which is how a
project brands the app for everyone who opens it. A workspace theme shadows a
global one of the same name, the same precedence skills and providers already
use, and it is [trust-gated](#trusting-a-workspace) — a cloned repository's
themes are not read until you have said the folder's config may be, because a
theme names a file path for its logo.

Editing a theme saves it back to the layer it came from, so a theme a repository
ships stays in the repository rather than being forked into your home directory
where the project can never see it again.

A file that will not parse costs itself and nothing else: the rest still load,
and what was wrong with it is reported in Settings › Appearance, naming the file,
the key and what to put there. Same for a colour that is not a colour, a logo
that is not there, or a size past its maximum — the theme paints the part of
itself that works.

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

## Tracing a turn

Off, and there is no default endpoint — not localhost, not a vendor. A harness
that reads private repositories has no business having an opinion about where a
description of that work should be sent, so an endpoint is a thing you type:

```json
{ "otlp_endpoint": "http://localhost:4318" }
```

That is OTLP over HTTP, which Langfuse, Phoenix, Jaeger, Grafana Tempo,
Honeycomb, and `docker run otel/opentelemetry-collector` all read. The spans are
named the way the OpenTelemetry GenAI semantic conventions name them, so those
tools read the *fields* too rather than showing a span called `chat` with an
opaque bag beside it:

```
invoke_agent  gen_ai.request.model=qwen3.6:27b  gen_ai.conversation.id=…
├─ chat            gen_ai.usage.input_tokens=1204  output_tokens=88
├─ execute_tool    gen_ai.tool.name=read_file
├─ execute_tool    gen_ai.tool.name=spawn_subagent
│  ├─ chat         …the delegate's own calls, nested
│  └─ execute_tool gen_ai.tool.name=grep
└─ chat            gen_ai.response.finish_reasons=stop
```

Delegation nesting is most of why this is worth having. A nine-step turn that
delegated twice reads as a tree instead of a flat list you reassemble by
timestamp, and the question it answers — *why did that take ninety seconds* — is
one no log line has ever answered well.

There is a local half that needs none of this. The same spans are kept in a
bounded ring in memory whether or not an endpoint is set, and the app's
**Traces** panel draws them — see [Where the time
went](working-with-it.md#where-the-time-went). It goes nowhere, holds no
message content, and is gone when you quit, which is the trade: it answers
*why was that turn slow* at the moment you are asking, and a collector is what
keeps the answer past today. Both can be on at once.

`OTEL_EXPORTER_OTLP_ENDPOINT` overrides the setting, because that is the
variable every other instrumented program reads and tracing one run should not
mean editing a file:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 taurus run "fix the flaky test"
```

**What is sent is the shape of a turn, not the turn.** Which model, how many
tokens, how long, which tools ran, what failed. Not the conversation. Carrying
that is a second setting and it is off:

```json
{ "otlp_capture_content": true }
```

Turn it on to debug why a model went the wrong way — reading the prompt it
actually got is the only way to answer that — and turn it off afterwards. It
sends the files it read, the commands it ran, and whatever you pasted in, to
whatever address is in the field above. See
[What a trace carries](safety.md#what-a-trace-carries).

A collector that cannot be reached is reported once, at startup, and changes
nothing else: the turn runs, the logs still work. The alternative — refusing to
start because a dashboard is down — would be a strange trade.

## Ollama, and the window nothing chooses for you

Ollama needs no configuration at all — a `base_url` and nothing else. It probes
its own models: tool support, vision, thinking, and the context window all come
back from `/api/show`, per model tag, remembered for the life of the provider.

One of those answers is not the answer to the question Taurus is asking. A model
reports the window it was **trained** for. What the machine in front of it can
serve at a speed anyone will wait for is a different number, and nothing on the
wire reports it. Left to itself, Ollama allocates the trained window, and on a
modern local model that is a KV cache far larger than the machine wants to hold.

Measured on `qwen3-coder:30b` — trained window 262,144 — with an ordinary
9,019-token agent prompt, warm, on one machine:

| Allocated | Prompt eval | Total | VRAM |
|---|---|---|---|
| 262,144 | 202.8s | 233.3s | 29.0 GB |
| 32,768 | 10.7s | 10.8s | 21.7 GB |

A turn is a dozen requests like that. The difference is between a local model
that works and one nobody waits for — and the symptom is never an error, just a
model that seems to have stopped thinking.

So `context_length` here is a **ceiling**, not a declaration:

```jsonc
[{ "id": "ollama", "base_url": "http://localhost:11434", "context_length": 65536 }]
```

Left unset it is 32,768, which after the reply's reserve is a working history of
roughly 24,500 tokens. Set it higher on a machine with room to spare, lower on one
without. A model trained for less than the ceiling keeps its own smaller window
either way: this only ever takes the smaller of the two, because asking for more
window than a model has is not a larger window, it is an error.

The same number is what compaction plans against, by construction — the request
allocates exactly the window the harness is filling. The two cannot come to
disagree, which matters more than it sounds: a harness planning for a window the
server was never asked to allocate fills a prompt the server then truncates from
the front, taking the system prompt and the tool definitions with it. The model
does not report that. It just gets worse.

## Anthropic and Google Gemini

Both are their own `kind` rather than a `base_url` pointed at a different host,
because neither is OpenAI-shaped. Anthropic reads the key from `x-api-key` by
default, puts the system prompt in a top-level field, and sends tool input as an
object;
Gemini calls the assistant `model`, gives tool calls no ids at all, and takes an
OpenAPI subset where the others take JSON Schema.

```jsonc
[
  { "id": "anthropic", "kind": "anthropic", "base_url": "https://api.anthropic.com" },
  { "id": "gemini", "kind": "gemini", "base_url": "https://generativelanguage.googleapis.com" }
]
```

That is the whole configuration. Keys go in the OS keychain as usual — `taurus
key set anthropic` — or in a variable named by `api_key_env`. Serving either
through a gateway needs two more fields; see
[Anthropic behind a gateway](#anthropic-behind-a-gateway).

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

## Anthropic behind a gateway

`kind: "anthropic"` speaks the Messages API, wherever that API is being served
from. On `api.anthropic.com` the route is fixed — the key rides `x-api-key` and
the endpoints sit under `/v1` — and neither field below needs a value.

A gateway in front of it changes both, so both are settings rather than
constants. A fixed header and a hardcoded `/v1` answer a correctly configured
Azure APIM route with a 401 or a 404, depending on which it trips over first.

```jsonc
{
  "id": "apim-claude",
  "kind": "anthropic",
  "base_url": "https://my-gateway.azure-api.net/claude",
  "api_prefix": "",
  "api_key_env": "APIM_SUBSCRIPTION_KEY",
  "api_key_header": "Ocp-Apim-Subscription-Key",
  "models": ["claude-opus-4-5", "claude-sonnet-4-5"],
  "default_model": "claude-opus-4-5"
}
```

- **`api_key_header`.** The key Taurus holds is the *gateway's*, not
  Anthropic's — an APIM route's own policy supplies the upstream key, which is
  most of the point of putting one there. Naming a header sends the key in it
  and nowhere else: sending both would hand a subscription key to Anthropic and
  an Anthropic key to the gateway, and one of the two would reject it. Left
  unset, the key rides `x-api-key` as it does at the API itself.
- **`api_prefix`.** An APIM API is published under a base path of its own, and
  its operations usually map straight onto `/messages` — so the prefix wants to
  be empty, and the base URL carries the whole path. Set it to `/v1` or leave it
  out for a gateway that mirrors Anthropic's own routes.
- **`models`.** Name them. A gateway need not proxy `/v1/models` at all, and
  Taurus never asks once this is set. Capability probing (`/v1/models/{id}`)
  degrades on its own — a route that will not answer falls back to a 200k window
  and vision on — so the listing is the only part that needs saying.

`anthropic-version` goes on every request whatever the key header is. A gateway
that injects its own is unharmed by receiving the same value, and one that
passes the request through needs it.

Two things a gateway cannot paper over. Reasoning blocks must come back with the
signature the provider issued them under, so a route that strips unknown fields
from responses will cause a rejected request one turn later, not at the moment
it strips them. And `kind: "anthropic"` is about the *wire format*, not the
vendor: a gateway exposing Claude through an OpenAI-shaped surface is
`kind: "open_ai_compatible"`, below.

Gemini has no equivalent yet: its key rides `x-goog-api-key` and its route is
fixed. If you need it behind a gateway, say so.

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
    {
      "id": "llama-3.1-8b",
      "context_length": 8192,
      "native_tools": false,
      "vision": false
    }
  ]
  ```

  The overrides matter because an OpenAI-compatible endpoint reports no
  capabilities at all, and one gateway commonly fronts models that share
  neither a context window, tool support, nor the ability to read an image —
  told the provider-wide 128000 above, an 8k model compacts tens of thousands
  of tokens too late. Anything left unset inherits the provider's own value, so
  a bare id means exactly what it means with no overrides at all.

  **Unset anywhere, a context window here is 128,000.** That is a guess, and it
  is the one number on this page that cannot be probed: `/v1/models` answers
  with ids and nothing else, whatever is behind it. It goes wrong in both
  directions and neither says so. Too high, and history is truncated from the
  front by a server that never reports it. Too low — a model with a larger
  window than the guess — and the harness compacts a conversation that had room
  to spare, over and over, so a session appears to fill up in a few turns and
  the summarizer runs on almost every one of them. If a model's window is not
  128k, say so here.

  A workspace layer *replaces* this list rather than adding to it. Appending
  could not express dropping a model, and a workspace that names models is
  saying which ones it wants.

- **`default_model`.** Which of them a new conversation starts on. Optional —
  the first model is used otherwise. It also still works alone, without
  `models`, which is all a single-model gateway ever needed. With neither, and
  no listing, the error says so instead of reporting an unreachable backend.

- **`vision`.** Whether attached images are sent. Defaults to true, because
  every model the hosted OpenAI API has served since gpt-4o reads them and
  nothing on the wire says so. Set it to `false` on a provider or a single
  model that fronts text-only weights: Taurus then refuses an attachment before
  the turn starts, naming the picture, rather than a round trip later with a
  wire error naming a field. Ignored by the other kinds — Ollama reports vision
  per model, and every model Anthropic and Gemini serve reads images.

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
