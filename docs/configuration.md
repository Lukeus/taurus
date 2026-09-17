# Configuration

<sub>[← Taurus AI Shell](../README.md)</sub>

The desktop app's **Settings** drawer edits providers, revokes permission
rules, and toggles skill and sub-agent synthesis. Everything it writes is a
plain file under `~/.taurus` that the CLI reads too, so the UI and a text
editor are interchangeable.

Every config file has two layers: the global `~/.taurus` and the workspace's
own `.taurus`. The workspace layer is read second and wins, the same
precedence skills use, **but only once you've said that workspace's config may
be read.** Until then only the global layer applies. See
[Trusting a workspace](#trusting-a-workspace).

Settings edits the **global** layer, and tells you when the current workspace
overrides a value on screen. That's deliberate: saving the merged view back
would write one project's overrides into the file every other project reads.

| File | Global | Workspace |
| --- | --- | --- |
| `providers.json` | Backends, including the header a key is sent in. Never the key itself, which lives in the OS keychain or an env var. | Overrides and additions for this project. |
| `mcp.json` | MCP servers over stdio or HTTP, in the same format Claude Desktop uses. Values may name env vars. The **MCP** panel reads and writes it; **Edit mcp.json** opens it. | Extra servers, or `{"disabled": true}` to switch an inherited one off. |
| `search.json` | Web search backends and which one is active. Never the key itself, which lives in the OS keychain or an env var, as with providers. | A different backend for this project, or field overrides on an inherited one. |
| `settings.json` | Last workspace, the two synthesis toggles, theme and theme id, fallback model, `max_iterations`. | The provider and model this project was last worked in, and a step limit for turns here. |
| `themes/` | Custom palettes, typefaces, wordmarks and corner radii. See [Themes](#themes). | Themes that travel with the project, so a repository can brand the app for everyone who opens it. |
| `skills/` | Skills available in every workspace. | Skills that travel with the project. |
| `permissions.json` | "Always everywhere" decisions. A file that doesn't parse grants nothing, is named in the log, and is never written over, so its rules are still there once you fix it. | "Always here" decisions, read the same way. |
| `sessions/` | Transcripts, in a directory per workspace. | — |
| `checkpoints/` | Pre-images of changed files, keyed by workspace like sessions and for the same reason. | — |
| `hooks.json` | Programs run at fixed points in a turn. | Extra hooks, or `{"disabled": true}` to switch an inherited one off. |
| `trust.json` | Which workspaces' own config may be read. Global only, because a repository that declared itself trusted would have declared nothing. A file that doesn't parse trusts nothing and is never written over. Trusting a folder then tells you which file to fix. | — |

## Trusting a workspace

The workspace layer isn't passive data:

- `mcp.json` starts child processes.
- `providers.json` names the endpoint your conversation is sent to.
- `search.json` decides whether `fetch_url` may reach private hosts.
- `permissions.json` is a standing grant.
- A skill can carry a script.

All of that travels in a repository you may have cloned a minute ago.

So there's one rule: **an untrusted workspace contributes no config at all.**
Your own `~/.taurus` still applies in full, so Taurus works normally in a fresh
clone. It just doesn't take instructions from the clone.

**Taurus asks nothing about a workspace that has no config of its own**, which
is most of them. The question only appears when the folder actually holds
something, and it lists what:

```
This project has configuration Taurus is not reading.
  1 skill
  1 MCP server
      probe: npx -y some-package
  2 standing permission grants — tools this project would allow without asking
```

MCP servers are listed by command line, not counted. That's the only part of
the list you can actually judge, and the part that starts a process on your
machine.

In the desktop app this is a banner above the composer, not a modal on open.
The decision isn't urgent: nothing from the folder is loaded, so nothing is
waiting on your answer. And a modal you must clear before starting work is how
a security prompt becomes a reflex. **Not now** dismisses it for that window
and records nothing.

In the terminal, every command prints one line when a workspace has config
going unread. You answer with `taurus trust`:

```
taurus trust             # what this workspace holds, and whether it is read
taurus trust --allow     # read it, from now on
taurus trust --revoke    # stop reading it
taurus trust --list      # every workspace trusted so far
```

Your answer takes effect immediately, not on the next launch. Trusting a
workspace loads its skills and agents and connects its servers. Revoking
unloads them and shuts the servers down.

In an untrusted workspace, the permission prompt offers **Allow once** and
**Deny** but not **Always here**. There's no workspace layer to keep a
standing decision in, and a button promising permanence it can't deliver would
be worse than none.

## Hooks

A hook is a program Taurus runs at a fixed point in a turn: a command line in
`hooks.json` that reads what's about to happen on stdin and answers with an
exit code. There's no API to build against and nothing to compile.

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

**A hook can refuse and can't permit.** `pre_tool_use` runs *after* the
permission engine has allowed a call. A hook can stop something you permitted
but never permit something you refused, so adding hooks only ever shrinks what
a machine will do.

That's deliberate. The alternative is a second permission system sitting
beside the first and disagreeing with it. It's also what makes a project's
hook file safe to honor at all once you trust the project.

| Event | When | Can it stop anything? |
| --- | --- | --- |
| `pre_tool_use` | Before a tool call, after permission | Yes |
| `post_tool_use` | After a tool call, pass or fail | No, the call has happened |
| `user_prompt_submit` | When you send a message | Yes |
| `stop` | When a turn ends | No |

`matches` narrows which calls a hook is about. Every field is optional.
Without `matches`, the hook applies to everything on its event.

- `commands` is keyed by the leading word of a command line, the same unit an
  "always allow" decision uses, so `git` never matches `rm`.
- `paths` globs against the workspace-relative paths a call names, taken from
  each tool's own declaration of what it touches. One glob covers every tool
  that writes.

Hooks run with **no shell**. `command` is a program and `args` are its
arguments. If you want a pipeline, put it in a script and name the script.

**What a hook is told:** one JSON object on stdin. Reading it is optional:

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
environment too, if your hook would rather not parse JSON. The working
directory is the workspace.

**What a hook says back:** its exit code, and nothing else. No JSON protocol
on stdout. A hook is usually three lines of shell, and a format it had to emit
*correctly* to be obeyed would sometimes come out wrong and be silently
ignored.

| Exit | Meaning |
| --- | --- |
| `0` | Fine. Anything on stdout reaches the model as a note. |
| `2` | Refused. stderr (or stdout) becomes the reason the model is given. |
| anything else | The hook didn't work. |

**A hook that can't run refuses.** A missing program, a crash, a timeout: on
`pre_tool_use` and `user_prompt_submit`, all of these deny. That's the
uncomfortable half, and it's on purpose. A hook exists to decide, and one that
couldn't hasn't approved anything.

A typo in `hooks.json` that blocks every call is loud, names the hook and the
exit code, and takes seconds to fix. A guard that silently stops guarding
never gets fixed, because nobody knows. On `post_tool_use` and `stop` there's
nothing left to stop, so a failure there is reported and the turn continues.

`timeout_seconds` defaults to 30 and stops at 600. A larger number is lowered
to ten minutes, not refused, because a hook refused at load doesn't run at
all. `taurus hooks list` shows the limit each hook actually gets.

A hook runs inside a turn, so a hook that hangs is a turn that hangs. A hook
that hits its limit is killed and, on events that can still refuse, counted as
a refusal. The limit covers everything, including handing the payload to a
hook that never reads its stdin.

Stop ends a running hook the same way, and cancels (not refuses) the call it
was guarding. `stop` hooks are the exception: they run once the turn is over,
stopped or not, so Stop has nothing left to end and only their own limit
bounds them.

The kill reaches the whole process tree, not just the program the hook names.
A script that calls a linter takes the linter with it.

- On Unix the hook runs in its own process group, and the group is signaled.
- On Windows it runs in a Job Object. Ending the job ends every process in it,
  including one whose own parent has already exited.

Neither reaches a process that leaves on purpose. See
[Known gaps](known-gaps.md).

To see what will run, and why something isn't:

```
taurus hooks list      # every hook that will run, and what narrows it
taurus hooks check     # entries that would not load, with the field named
```

Hooks follow the trust gate like every other layered file. A workspace's own
`hooks.json` does nothing until you trust that workspace. See
[Trusting a workspace](#trusting-a-workspace).

## API keys

A key never goes in a config file. Type it into Settings and it goes to the OS
credential store, under the service `taurus` and the provider's id:

- Keychain on macOS
- Credential Manager on Windows
- the Secret Service on Linux

Web-search backends use the same store under `search:<id>`, since you choose
both kinds of id and a backend and a provider may well share a name.

Or from a terminal:

```bash
taurus key set openai        # prompts, input not echoed
taurus key set openai < key.txt
pass show openai | taurus key set openai
taurus key set brave --search   # the same, for a web-search backend
taurus key status            # where every key comes from, both kinds
taurus key clear openai
```

The key is read from stdin, never from an argument. A key on the command line
is visible to every process on the machine through `ps`, and it lands in the
shell history of whoever typed it.

**An environment variable wins over a stored key.** Exporting one is an
explicit act, usually in CI or a container with no keychain at all. If a
stored key silently beat it, headless runs would be unpredictable.

So `api_key_env` is optional. Name a variable and it wins. Leave it unset and
the stored key is used. `taurus key status` and the Settings field both show
which is in effect. Otherwise "I stored a key and it isn't being used" is a
401 that explains nothing:

```
$ taurus key status
ollama               none
openai               $OPENAI_API_KEY  (a stored key is being overridden)
azure                keychain
```

Settings never shows a stored key, only where it comes from. The field is for
typing a new one, not reviewing the old one. A secret handed to the webview
lives in JavaScript memory and whatever the DOM does with it, and nothing on
that screen needs the value.

Two platform details:

- On macOS the keychain grants access per binary. The first time the `taurus`
  CLI reads a key the desktop app stored (or the reverse), the OS asks you to
  allow it. "Always Allow" makes that once. An update is a new binary, so it
  can ask again. Loading never waits on that dialog: Taurus checks that a key
  is saved without reading it, and reads it the first time it's used (a
  provider's on its first request, a search backend's on the first search).
- On Linux the Secret Service is a running D-Bus service, not a file, and a
  headless box may have none. Then storing fails, `taurus key status` says so,
  and environment variables are the whole story.

## MCP servers

**Browse servers** in the MCP panel has the setup for the servers Taurus
knows: the command, which argument takes the directory, which header holds the
token. Adding GitHub doesn't start with reading a README.

Filling one in produces an ordinary entry and hands it to the same form you'd
use to type one by hand. You see the command line before anything is written,
and **Test** is the same Test.

![The catalog, listing the servers Taurus knows the setup
for](screenshots/mcp-catalog.png)

Adding one writes into `mcp.json` and nothing else. Nothing is downloaded and
no installer runs: `npx` and `uvx` fetch the program at launch, as they do for
an entry you typed. The blast radius of the button is a config file.

The list ships with the app instead of being fetched. Somebody reviewed each
entry in a commit against its source, which each card links.

That's the whole difference from a registry search. The reviewable part of
`npx -y @scope/package` is a package name, which says nothing about what the
package does. A search box returning those would ask for a decision nobody in
the loop can make. The costs:

- The list goes out of date between releases. The panel shows when it was last
  checked.
- A server not on it is added by hand: one extra step, not a dead end.

A stale list can't break a working setup. Installing copies the entry into
`mcp.json`, and the catalog never looks at it again.

**Some of what you'll search for isn't there, and says why.** Postgres has had
no first-party server since the reference one was archived and deprecated over
a SQL-injection vulnerability, and nothing official replaced it. Entries like
that show the reason instead of a button, so searching for something you
can't have gets an explanation, not nothing.

### Signing in

Hosted servers (Linear, Google Drive, and most vendor-run ones) use OAuth, not
a token you can paste. Add the entry, then press **Sign in** on its card.
Taurus discovers the authorization server, registers itself, and opens your
browser. Approving there sends the browser back to a loopback address Taurus
listens on for exactly one request. The tokens land in the OS keychain, like
provider API keys, and never in `mcp.json`.

Nothing starts that flow on its own. A browser window opening because a server
answered 401 would be the app taking over your screen for something you didn't
do. A server that needs an account says so and waits to be asked.

Every request carries a token minted for it, refreshed once it has expired, so
a window left open overnight keeps working with no failed call after the token
lapses. **Sign out** forgets Taurus's copy. The grant itself stays until you
remove the app in the provider's own settings, the only place it can be
revoked.

Stdio servers have no sign-in and aren't offered one. The MCP authorization
spec says a local program takes its credentials from the environment, which is
what `${VAR}` below is for.

**A credential goes in the global file by default.** Cataloged entries that
want one default to `~/.taurus/mcp.json`, which nobody commits. Choosing this
project writes it into `<workspace>/.taurus/mcp.json` instead, a file one
`git add .` from being published, so Taurus asks you to confirm. It isn't
refused: there are good reasons to want it.

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

`${VAR}` is read from the environment wherever a value appears: a URL, a
header value, a stdio `command`, an argument, an `env` value. A server almost
always needs a credential, and the workspace `mcp.json` is meant to be
hand-written and version-controlled, so a literal token there is a token in
the repository. `providers.json` makes the same bargain for API keys.

An unset variable fails the server with the variable's name in the message.
Taurus doesn't send an empty `Authorization` header and get a 401 that looks
like a bad token instead of a missing one. A literal value passes through
untouched.

### The MCP panel

**MCP** in the rail lists every configured server across both layers: what
each one is, whether it connected, and what it offers. Adding, editing,
enabling, and removing each write one entry of `mcp.json`. The rest of the
file is copied through untouched, including keys this version doesn't model.
So a server added in the panel and one pasted by hand are the same thing, and
neither route disturbs the other. **Edit mcp.json** covers anything the form
can't express.

**Test** connects the entry in front of you, reports the tools it found, and
disconnects. It registers nothing and leaves any live connection alone, so you
can check an edit before you save it.

Each connected server shows its cost, such as **~1.2k tokens of every
request**, and the header shows a total for the drawer beside the tool count.
That's the number the switch next to it is for.

You pay for a tool on every iteration of every turn, called or not. Four
servers left on out of habit are four servers the conversation starts out
owing. On an 8k window, that's the difference between a harness that works and
one with no room left to think in. It's the Context panel's fixed-half figure
narrowed to one server, from the same arithmetic, so the two can't disagree.

A server that never connected shows no figure, not a zero: its tools aren't
registered, so there's nothing to measure. The figure is what enabling a
server cost you, not what it would cost. A connected server that offers
nothing shows a real `~0`, which is worth saying out loud: it's a child
process running for no benefit.

A stored value that isn't a `${VAR}` reference is treated as a secret. The
panel is told the key is set but never gets the value. Leave the field alone
to keep what's on disk, or type over it to replace it.

Saving reconnects the MCP servers and nothing else. Skills, providers, and the
index are untouched, the same rule agent edits follow. The one exception is the
agent roster, and only when a save changes which tools the servers offer. An
agent can be scoped to an MCP tool, so adding its server has to make that agent
usable, and deleting the server has to stop the roster claiming a tool that's
gone.

Saving restarts the server you saved, plus any other server whose entry changed.
Everything else keeps running. The same goes for switching folders and for
trusting or revoking one: a server only restarts when its merged entry is
different, so your global servers carry on across a switch. "Different" means
what would actually start. A `${VAR}` whose value changed counts, and so does a
folder's own entry for a server with the same name. The folder itself doesn't:
a server starts in Taurus's own working directory, and nothing tells it which
folder is open. A server that isn't connected, because it never started or
stopped answering, is always started again. **Reconnect** restarts every
server regardless, which is what you want for one that's hung. Signing in or
out restarts that server, since a connection keeps the credentials it opened
with.

### When a server will not start

The most common failure isn't a wrong entry. An app launched from the Dock or
Finder inherits the launcher's environment. On macOS that PATH is
`/usr/bin:/bin:/usr/sbin:/sbin`: no Homebrew, no nvm, no pyenv, no
`~/.local/bin`. `npx` and `uvx` live in exactly those places, so a correct
entry for an installed program fails with "command not found".

Taurus asks your login shell for its PATH once at startup and merges what it
finds, which fixes this for most setups. It uses `-l -i`, because nvm and pyenv
install themselves into `.zshrc`, not `.zprofile`.

The panel's **Program search path** section shows the result:

- the directories being searched
- which of them the shell contributed
- when a server's command isn't among them, that it couldn't be found

Set `TAURUS_SKIP_LOGIN_PATH=1` to skip the probe. A program named by its full
path doesn't need it.

The panel also tells you, so you don't have to find out:

- A server that won't parse is reported **by name, with the key that's
  wrong**, and its neighbors still load. One typo doesn't discard every
  server in the file.
- A server switched off is listed as `off`, not hidden.
- A server that never answers is given up on after 60 seconds, so one hung
  program can't stall the reload the others are waiting for.
- A server that **stops** answering partway through a session goes back to
  red with the reason, instead of staying listed as connected while every
  call against it fails. Press **Reconnect** to start it again.

### What a call may take

Every tool call an MCP server serves has two limits, because a server is a
program nobody here reviewed, running in the same window as everything else.

**Two minutes of silence.** A call silent for two minutes is given up on, and
the model is told the server either hung or reports no progress. The clock
measures *silence*, not work. Every progress notification restarts it, so a
job that legitimately takes an hour and says so is never cut off.

There's no ceiling beyond that, since Taurus can't usefully guess how long
your build takes. **Stop** ends a call at any point, and tells the server to
stop instead of just walking away from the answer.

**A share of the context.** A result too large for the window is cut to its
head and tail, with a line saying how many bytes went and where. The whole
answer is written where long command output goes, so the middle is a
`read_file` away, not lost. Pictures are never cut, since half an image is a
broken image. The share is the same one the shell tool takes.

### What the agent may and may not do

**The agent can draft an entry but never install one.** `draft_mcp_server`
takes a name and a command line and hands back a block to paste, the file it
belongs in, and what you must fill in first. It writes nothing and starts
nothing.

That differs from skills and sub-agents on purpose. Those are reviewable: what
you approve is the text that will run. An MCP entry points at code nobody in
the loop has seen (the reviewable part of `npx -y @scope/package` is a package
name), and that program runs at every launch, before any tool call, outside
the permission engine. A review card there would ask for a decision with the
information missing. So the model does what it's good at, knowing the
server's name and which arguments it takes, and installing stays with you.

The panel doesn't change that. It's a form *you* fill in, the agent can't
reach it, and drafting still writes nothing and starts nothing. It changes
where you install: in a window that can tell you the entry is malformed, the
program isn't on the PATH, or the server doesn't answer. A text editor can
tell you none of that.

Secrets are never carried through the draft. `env` and `headers` take variable
and header *names*. The block has `<replace-me>` where each value goes, and the
model is told to explain each one, not guess at it. A key the model typed would
live in the transcript, and every copy of it, for as long as the conversation
is kept. The block is rendered through the same type the loader reads, so what
comes back will parse.

## Themes

The window ships two palettes and follows your system between them. A
**theme** sets whose colors, typefaces and wordmark they wear. It's a file in
`~/.taurus/themes/`. Settings › Appearance edits it, but you can also write one
by hand.

![Settings, Appearance](screenshots/appearance.png)

A theme supplies fourteen colors, three typefaces, a wordmark and a corner
radius, and nothing else. It can be that small because `src/styles.css` names
its raw values exactly once and speaks in *roles* everywhere else: a panel is
`--bg-raised`, a hairline is `--rule`, the lead accent is `--accent`. Fourteen
colors at the top move six thousand lines below them.

There's no stylesheet, selector or length beyond those. A theme that could
restate a rule could break a layout in a way only its author could reproduce.

### The file

Everything is optional. The common case is a different accent, which takes
four lines:

```json
{
  "name": "Midnight",
  "dark": { "accent": "#b48cff", "accent-hover": "#c9aaff" }
}
```

Anything you leave out falls through to the shipped palette, which also keeps
a theme written today working after the app adds a token tomorrow. A fuller
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
| `dark`, `light` | The fourteen colors, by the names in the table below. Hex only: `#rgb`, `#rrggbb` or `#rrggbbaa`. |
| `fonts` | `display`, `body`, `mono`. A family name, not a stack. The fallbacks after it stay the app's, so naming a font you don't have degrades instead of breaking. It has to be **installed on the machine**. The window loads no remote stylesheets, so a theme can't bring a typeface with it. |
| `brand.wordmark` | The word beside the mark. An empty string is a real answer and means a mark on its own. Leaving the key out keeps `taurus`. |
| `brand.logo` | An SVG, PNG, JPEG or WebP up to 256KB. A bare name is read from the theme file's folder, so a logo committed beside it travels with it. |
| `shape.radius` | Multiplier on the corner-radius ladder, 0 to 3. `0` is square, `1` is as shipped. |
| `shape.gutter`, `shape.rail-gutter` | The two column insets, in px, up to 96. |

The color names are *jobs*, not colors. The design system names its accents
after what they happen to be (cyan, peach, mint), which is fine with one
palette and absurd in a file whose whole point is that the accent might be
violet.

| Name | Where it is |
| --- | --- |
| `ink` | The window itself, and the native ground behind the webview. |
| `surface-1` | A panel raised off it: the rail, a drawer, a card. |
| `surface-2` | A panel raised off that, and the active state of a row. |
| `surface-hover` | The step between, so a hover reads as on the way to a selection, not as one. |
| `line` | The one hairline weight. |
| `text`, `text-dim`, `text-faint` | Three weights, brightest first. The faint one carries the 10px mono micro-labels. |
| `accent`, `accent-hover` | The lead color. |
| `on-accent` | What stays legible on top of it, like a filled button's label. |
| `ok`, `warn`, `danger` | The three signals. |

### Dark, light, or both

`dark` and `light` are separate palettes, not one palette with a base. Fill in
both and the System/Light/Dark choice keeps working under your brand. "Follow
the system" is a preference people keep, and a theme that couldn't honor it
would quietly take it away.

Fill in only one and you've said *this brand is dark*. Selecting it pins the
mode, and the three pills say why instead of seeming to do nothing. A theme
that only changes the typeface and wordmark names neither palette, and is as
good in daylight as at night.

### Contrast

The editor measures every pair the app actually puts on screen:

- body text on each of the three surfaces
- the faint labels
- the accent
- the label on a filled button
- the three signals

It names each pair below 4.5:1 by where it is on screen, not by which two
tokens it's between.

It warns, it doesn't refuse. WCAG's floors are the right default, but it's
your machine and your screen, and a checker that blocked saving a 4.2:1 would
be enforcing a taste. What it won't do is let it happen silently, which is
where a branding feature ends up if nobody builds this.

### Where a theme can live

Both config layers, like everything else here. `~/.taurus/themes/` is yours.
`.taurus/themes/` inside a workspace travels with the repository, which is how
a project brands the app for everyone who opens it.

A workspace theme shadows a global one of the same name, the precedence skills
and providers use. It's [trust-gated](#trusting-a-workspace) too: a theme
names a file path for its logo, so a cloned repository's themes aren't read
until you've said the folder's config may be.

Editing a theme saves it back to the layer it came from. A theme a repository
ships stays in the repository instead of being forked into your home
directory, where the project could never see it again.

A file that won't parse costs only itself. The rest still load, and Settings ›
Appearance says what's wrong, naming the file, the key and what to put there.
The same goes for a color that isn't a color, a missing logo, or a size past
its maximum: the theme paints the part of itself that works.

## Web search

Two tools, `web_search` and `fetch_url`, and **neither exists until you turn
one on.** Searching sends your prompt to a third party, which shouldn't start
just because you installed a program.

In the app, open **Settings › Search**, pick a backend, and paste its key. The
key goes to the OS keychain like provider keys, and the tools are registered
the moment the backend resolves. No restart.

It all lands in `~/.taurus/search.json`, which the CLI reads too. A first run
leaves it with every backend spelled out and none selected:

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

`api_key_env` also works and **wins over a stored key** when both are set,
which is what makes CI and keychain-less machines work. The precedence matches
providers because both resolve through the same code.

| `kind` | Needs | Notes |
| --- | --- | --- |
| `brave` | An API key | Free tier covers ordinary use. |
| `tavily` | An API key | Built for agents. Returns page extracts, not one-line snippets. |
| `searxng` | A `base_url` | No key and no account. Your instance has to enable the `json` format in its `settings.yml`, or it answers API requests with a 403. |

Backends layer field by field, like providers. So a workspace can change one
thing without restating the rest:

- retarget one setting:
  `{"backends": {"brave": {"base_url": "http://proxy.internal"}}}`
- switch which one is active, with a bare `{"backend": "searxng"}`

`base_url` may name an environment variable the same way `mcp.json` does.

Both tools carry the `network` effect, so both prompt with what they'd send:
`web_search` the full query, `fetch_url` the whole URL, never abbreviated.
You're approving which host the request goes to, and a shortened URL is
exactly what hides it.

Redirects are followed only while the host stays the same. One approval is
worth exactly one host, and the default policy would spend it elsewhere:
approve a link shortener and the hop lands wherever it points, including on
your own network. A redirect that crosses hosts stops and reports its target,
which the model can then ask for on its own terms.

**`fetch_url` won't reach your own machine or network.** Every address the
host resolves to has to be public. So `http://127.0.0.1:8080/admin` is
refused, and so is `http://169.254.169.254/`, the cloud metadata endpoint,
which answers unauthenticated and with credentials.

The resolution happens inside the HTTP client `fetch_url` uses, so the
connection gets exactly the addresses that were checked. There's no second
lookup for a name to answer differently.

The URL there is chosen by a model that just read a web page. Search backends
and MCP servers are different: those are addresses you wrote down, and this
check never applies to them. To have the model read a docs server you run
locally, opt in on purpose:

```jsonc
{ "allow_private_hosts": true }
```

It's file-only on purpose, not a Settings checkbox. It's the one setting here
where the easy version of the mistake is expensive, and a config file makes
you think for a moment.

The tools are registered together or not at all. A `fetch_url` with no way to
find a URL only works on links you paste, and search without fetch leaves the
model holding snippets it can't follow.

If the selected backend can't run (no key saved and none in the environment,
or a SearXNG entry with no URL), nothing is registered and Settings › Search
says why. The model doesn't spend a turn discovering it has no credential. The
tab also says outright when a picked backend still isn't running, because a
selection alone isn't a working backend.

## Tracing a turn

Tracing is off, with no default endpoint: not localhost, not a vendor. A
harness that reads private repositories has no business deciding where a
description of that work goes, so you type the endpoint:

```json
{ "otlp_endpoint": "http://localhost:4318" }
```

That's OTLP over HTTP, which Langfuse, Phoenix, Jaeger, Grafana Tempo,
Honeycomb, and `docker run otel/opentelemetry-collector` all read. Spans
follow the OpenTelemetry GenAI semantic conventions, so those tools read the
*fields* too instead of showing a span called `chat` with an opaque bag beside
it:

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
delegated twice reads as a tree, not a flat list you reassemble by timestamp.
It answers *why did that take ninety seconds*, which no log line has ever
answered well.

There's also a local half that needs none of this. The same spans sit in a
bounded ring in memory whether or not an endpoint is set, and the app's
**Traces** panel draws them. See
[Where the time went](working-with-it.md#where-the-time-went).

The ring goes nowhere, holds no message content, and is gone when you quit.
That's the trade: it answers *why was that turn slow* while you're asking, and
a collector keeps the answer past today. Both can be on at once.

`OTEL_EXPORTER_OTLP_ENDPOINT` overrides the setting. Every other instrumented
program reads it, and tracing one run shouldn't mean editing a file:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 taurus run "fix the flaky test"
```

**What's sent is the shape of a turn, not the turn.** Which model, how many
tokens, how long, which tools ran, what failed. Not the conversation. Sending
that takes a second setting, and it's off:

```json
{ "otlp_capture_content": true }
```

Turn it on to debug why a model went the wrong way, since reading the prompt it
actually got is the only way to answer that, then turn it off. It sends the
files the model read, the commands it ran, and whatever you pasted in, to
whatever address is in the field above. See
[What a trace carries](safety.md#what-a-trace-carries).

An unreachable collector is reported once, at startup, and changes nothing
else: the turn runs and the logs still work. Refusing to start because a
dashboard is down would be a strange trade.

## Ollama, and the window nothing chooses for you

Ollama needs nothing but a `base_url`. Its models are probed: tool support,
vision, thinking, and the context window all come back from `/api/show`, per
model tag, remembered for the life of the provider.

The context window it reports is the wrong number for Taurus. A model reports
the window it was **trained** for. What your machine can serve at a speed
anyone will wait for is a different number, and nothing on the wire reports
it. Left alone, Ollama allocates the trained window, which on a modern local
model is a KV cache far larger than the machine wants to hold.

Measured on `qwen3-coder:30b` (trained window 262,144) with an ordinary
9,019-token agent prompt, warm, on one machine:

| Allocated | Prompt eval | Total | VRAM |
|---|---|---|---|
| 262,144 | 202.8s | 233.3s | 29.0 GB |
| 32,768 | 10.7s | 10.8s | 21.7 GB |

A turn is a dozen requests like that. That's the difference between a local
model that works and one nobody waits for, and the symptom is never an error,
just a model that seems to have stopped thinking.

So `context_length` here is a **ceiling**, not a declaration:

```jsonc
[{ "id": "ollama", "base_url": "http://localhost:11434", "context_length": 65536 }]
```

Unset, it's 32,768: a working history of roughly 24,500 tokens after the
reply's reserve. Set it higher on a machine with room to spare, lower on one
without. A model trained for less than the ceiling keeps its own smaller
window either way. Taurus only ever takes the smaller of the two, because
asking for more window than a model has gets you an error, not a larger
window.

Compaction plans against the same number, by construction: the request
allocates exactly the window the harness is filling, so the two can't
disagree. That matters more than it sounds. A harness planning for a window
the server was never asked to allocate fills a prompt the server truncates
from the front, taking the system prompt and the tool definitions with it. The
model doesn't report that. It just gets worse.

## Anthropic and Google Gemini

Each is its own `kind`, not a `base_url` pointed at a different host, because
neither is OpenAI-shaped:

- Anthropic reads the key from `x-api-key` by default, puts the system prompt
  in a top-level field, and sends tool input as an object.
- Gemini calls the assistant `model`, gives tool calls no ids at all, and takes
  an OpenAPI subset where the others take JSON Schema.

```jsonc
[
  { "id": "anthropic", "kind": "anthropic", "base_url": "https://api.anthropic.com" },
  { "id": "gemini", "kind": "gemini", "base_url": "https://generativelanguage.googleapis.com" }
]
```

That's the whole configuration. Keys go in the OS keychain as usual
(`taurus key set anthropic`) or in a variable named by `api_key_env`. Serving
Anthropic through a gateway needs two more fields. See
[Anthropic behind a gateway](#anthropic-behind-a-gateway).

**Neither needs a `context_length`,** and you should only give one as a
fallback. Anthropic reports a window and a capability tree per model, so
Taurus asks. Gemini reports a window in its model listing.

Each answer is remembered per model for the life of the provider. Compaction
asks once per iteration of the agent loop and the number can't change mid-turn,
so probing each time would put a round trip before every model call. A
configured value that disagrees with the model makes a conversation compact at
the wrong moment. So Settings offers the field as "only used if the backend
will not report its own window" and leaves it empty by default.

**Prompt caching is on by default on Anthropic.** The system prompt and tool
schemas are exactly the fixed overhead [`taurus usage`](working-with-it.md#the-context-window)
exists to report, re-sent on every iteration of every turn. Anthropic is the
one backend here that serves them back at about a tenth of the price.

Taurus uses two of the four allowed breakpoints:

- one after the system prompt, which also covers the tools rendered before it
- one on the newest turn, so the cached prefix grows with the conversation
  instead of resetting each iteration

Cached tokens count toward the input total, so a well-cached turn reports what
the request carried, not only the part that missed.

**Thinking is left to the model by default.** Sending no `thinking` field is
the only setting valid on every model that API has served: newer models reason
by default, older ones don't, and neither rejects a request that says nothing.
`"thinking": "adaptive"` or `"disabled"` overrides it. The wrong one is a 400,
not a preference, which is why Taurus doesn't guess.

Reasoning blocks are replayed with the signature the provider issued them
under. That's not a nicety: a turn that reasoned and then called a tool is
only legal on the next request if its thinking comes back signed and unedited.
A signature lost in the stream means a rejected request one turn later.

**Gemini's schemas are sanitized on the way out.** Gemini accepts an OpenAPI 3
subset and refuses a request outright on a keyword it doesn't know, with an
error naming the tool, not the offending word. So `$schema`, `title`,
`additionalProperties`, and the integer-width `format`s that `schemars` emits
are stripped at every level of every tool schema.

Gemini's tool calls carry no ids, so Taurus synthesizes them and resolves them
back to names on the way out. Without that, two calls to the same tool in one
turn, and their results, would be indistinguishable.

## Anthropic behind a gateway

`kind: "anthropic"` speaks the Messages API wherever it's served from. On
`api.anthropic.com` the route is fixed (the key rides `x-api-key` and the
endpoints sit under `/v1`), and neither field below needs a value.

A gateway in front changes both, so both are settings, not constants. A fixed
header and a hardcoded `/v1` would get a 401 or a 404 from a correctly
configured Azure APIM route, depending on which it tripped over first.

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
  Anthropic's: an APIM route's own policy supplies the upstream key, which is
  most of the point of putting one there. Naming a header sends the key in it
  and nowhere else. Sending both would hand a subscription key to Anthropic and
  an Anthropic key to the gateway, and one of the two would reject it. Unset,
  the key rides `x-api-key`, as it does at the API itself.
- **`api_prefix`.** An APIM API is published under its own base path, and its
  operations usually map straight onto `/messages`. So the prefix should be
  empty and the base URL carries the whole path. Use `/v1`, or leave it out,
  for a gateway that mirrors Anthropic's own routes.
- **`models`.** Name them. A gateway doesn't have to proxy `/v1/models` at
  all, and Taurus never asks once this is set. Capability probing
  (`/v1/models/{id}`) degrades on its own: a route that won't answer falls
  back to a 200k window with vision on. So the listing is the only part you
  need to state.

`anthropic-version` goes on every request, whatever the key header. A gateway
that injects its own isn't harmed by the same value, and one that passes the
request through needs it.

Two things a gateway can't paper over:

- Reasoning blocks must come back with the signature the provider issued them
  under. A route that strips unknown fields from responses causes a rejected
  request one turn later, not at the moment it strips them.
- `kind: "anthropic"` is about the *wire format*, not the vendor. A gateway
  exposing Claude through an OpenAI-shaped surface is
  `kind: "open_ai_compatible"`, below.

Gemini has no equivalent. Its key rides `x-goog-api-key` and its route is
fixed. If you need it behind a gateway, say so.

## Azure OpenAI, and gateways in front of it

Azure is an OpenAI-compatible backend that disagrees about one thing: where the
key goes.

- OpenAI and everything imitating it read `Authorization: Bearer`.
- Azure OpenAI reads `api-key`.
- An Azure API Management gateway reads `Ocp-Apim-Subscription-Key`.

Both Azure headers take the key bare. A `Bearer ` in front of the value gets a
401 that looks exactly like a wrong key.

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

Unset, it keeps bearer auth and nothing else changes. There's deliberately no
setting for the scheme: naming `Authorization` sends the key bare in that
header, the only other shape a gateway asks for. The value is marked
sensitive, so a subscription key can't reach a debug log through the header
map.

Other fields matter more here than elsewhere:

- **`api_prefix`.** Azure's OpenAI-shaped surface lives under `/openai/v1`,
  where the model goes in the request body. Its older data plane puts the
  deployment name in the path and requires an `api-version` query parameter.
  Taurus can't express either, so point it at the `/openai/v1` route, or at an
  APIM route whose policy supplies them.
- **`models`.** A gateway doesn't have to expose `/v1/models`, and plenty that
  do answer with an inventory, not an entitlement: every model the vendor
  sells, including ones this key can't call. Naming models here replaces that
  listing outright. The picker offers what's listed, and no request is made to
  find out. Leave it out and Taurus asks, which is right for Ollama and any
  endpoint that answers usefully.

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
  neither a context window, tool support, nor the ability to read an image.
  Told the provider-wide 128000 above, an 8k model compacts tens of thousands
  of tokens too late. Anything unset inherits the provider's own value, so a
  bare id means exactly what it means with no overrides.

  **Unset anywhere, a context window here is 128,000.** That's a guess, and
  the one number on this page that can't be probed: `/v1/models` answers with
  ids and nothing else, whatever is behind it. It goes wrong in both
  directions, silently. Too high, and a server truncates history from the
  front without reporting it. Too low (a model with a larger window than the
  guess), and the harness keeps compacting a conversation that had room to
  spare. A session seems to fill up in a few turns, and the summarizer runs on
  almost every one. If a model's window isn't 128k, say so here.

  A workspace layer *replaces* this list, it doesn't add to it. Appending
  couldn't express dropping a model, and a workspace that names models is
  saying which ones it wants.

- **`default_model`.** Which of them a new conversation starts on. Optional:
  the first model is used otherwise. It also works alone, without `models`,
  which is all a single-model gateway needs. With neither, and no listing, the
  error says so instead of reporting an unreachable backend.

- **`vision`.** Whether attached images are sent. Defaults to true: every
  model the hosted OpenAI API has served since gpt-4o reads them, and nothing
  on the wire says so. Set it to `false` on a provider or a single model that
  fronts text-only weights. Taurus then refuses an attachment before the turn
  starts, naming the picture, instead of a round trip later with a wire error
  naming a field. Other kinds ignore it: Ollama reports vision per model, and
  every model Anthropic and Gemini serve reads images.

## Intel hardware, and other backends

Taurus never touches an accelerator. Every provider crate is JSON over HTTP,
with no inference code and no device selection in this repository. Whether
your CPU, GPU, or NPU runs a model is up to the server you point it at.

That matters because **stock Ollama on Intel runs on the CPU.** Intel
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

Models must be in OpenVINO IR format. Export with
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

Three settings decide whether this works. Getting them wrong fails in ways
that don't look like configuration errors:

- **`api_prefix`.** Before 2026.3, OVMS serves the OpenAI routes only under
  `/v3`. 2026.3 adds `/v1` as an alias. On anything earlier, add
  `"api_prefix": "/v3"`, or you get a flat `404` from a server that's running
  fine.
- **`context_length`.** OVMS caps prompts at 8k tokens on NPU. This value
  drives compaction and can't be probed over the OpenAI API, so it defaults to
  128000, roughly sixteen times too high for an NPU. History would never be
  compacted before the server began rejecting requests, and the failure shows
  up as a provider error, not as "this conversation got too long".
- **`native_tools`.** Tool calling in OVMS needs a per-model *tool parser*.
  Qwen3, Hermes3, Llama3, Mistral-7B-v0.3, phi-4-mini and others have one. A
  model served without one, with `native_tools` left at its default of true,
  narrates tool calls it never actually makes. Set it to `false` and Taurus
  switches to prompted tool calling, the same fallback it uses for `gemma3` on
  Ollama.

`api_prefix` isn't OpenVINO-specific. Any server behind a reverse proxy that
mounts the API elsewhere needs it. `""` puts the routes directly on the base
URL.

Layering is per key, not per file, so an override states only what it changes:

```jsonc
// <workspace>/.taurus/providers.json — point one provider at a different box
[{ "id": "ollama", "base_url": "http://gpu-box:11434" }]
```

`kind`, `api_key_env`, `api_key_header`, and the capability overrides are
inherited from the global entry with the same `id`. An entry whose `id` is new
to this layer is added instead, and needs its own `kind` and `base_url`.

Anything unresolvable (a malformed layer, an override with no `kind`, an MCP
toggle naming a server nothing defines) is reported as a startup problem, not
silently dropped, and the other layer still loads.

The workspace layer belongs to a directory you can change at any time, so all
of it is re-resolved on every workspace switch, not just at startup.

Two values are deliberately not layered:

- `last_workspace` is written globally only. A pointer to a workspace, stored
  inside the workspace it names, could only ever point at itself.
- Edits from the app's provider settings also write globally. The workspace
  layer is meant to be hand-written and version-controlled, and round-tripping
  the merged view into it would bake every inherited value into the project
  file.
