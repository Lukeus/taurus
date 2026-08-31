/**
 * Turning MCP server entries into things a form can edit, and back.
 *
 * Kept out of the components because two of these are the kind of thing that is
 * wrong in a way nobody notices until a server will not start: splitting a
 * command line, and deciding what a server's state actually is when it is
 * configured, switched off, connected, and broken in four different
 * combinations.
 */
import type {
  CatalogEntry,
  McpServerDraft,
  McpServerView,
  McpTransport,
  McpValue,
  Scope,
} from "./api";

/**
 * Splits a pasted command line the way a shell would, minus the parts a shell
 * does for a shell's reasons.
 *
 * Quotes and backslash escapes are honoured because a server argument is a path
 * often enough — `--root "/Users/me/My Documents"` — and splitting that on
 * spaces produces two arguments and a server that starts in the wrong place. No
 * variable expansion, no globbing, no operators: what goes into `args` is passed
 * to the program directly, with no shell in between, so pretending otherwise
 * here would produce an entry that behaves differently from the one it looks
 * like.
 */
export function splitCommandLine(text: string): string[] {
  const out: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;
  let started = false;

  for (let i = 0; i < text.length; i++) {
    const char = text[i];

    if (char === "\\" && quote !== "'" && i + 1 < text.length) {
      current += text[++i];
      started = true;
      continue;
    }
    if (quote) {
      if (char === quote) quote = null;
      else current += char;
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      // An empty quoted string is an argument: `--flag ""` means one empty
      // argument, not nothing at all.
      started = true;
      continue;
    }
    if (/\s/.test(char)) {
      if (started) out.push(current);
      current = "";
      started = false;
      continue;
    }
    current += char;
    started = true;
  }
  if (started) out.push(current);
  return out;
}

/** The inverse, quoting only what would not survive the trip back. */
export function joinCommandLine(parts: string[]): string {
  return parts
    .map((part) =>
      part === "" || /[\s"'\\]/.test(part)
        ? `"${part.replace(/(["\\])/g, "\\$1")}"`
        : part,
    )
    .join(" ");
}

/** An empty entry, for the Add form. */
export function blankDraft(scope: Scope): McpServerDraft {
  return {
    name: "",
    scope,
    transport: "stdio",
    command: "",
    args: [],
    env: [],
    url: "",
    headers: [],
    disabled: false,
  };
}

/**
 * A saved server as a draft the form can edit.
 *
 * The values come across as they were sent — a `${VAR}` reference in full, a
 * literal held back and marked — and go back the same way, which is what lets a
 * save leave a token it never saw alone.
 */
export function draftFrom(view: McpServerView): McpServerDraft {
  return {
    name: view.name,
    scope: view.scope,
    transport: view.transport,
    command: view.command,
    args: view.args,
    env: view.env,
    url: view.url,
    headers: view.headers,
    disabled: view.disabled,
  };
}

/**
 * What a row shows about a server, in the order the question is actually asked:
 * is it on, did it connect, and what did it give us.
 *
 * One function rather than a chain of ternaries in the card, because the four
 * states overlap — a disabled server has a stale status, a server that has never
 * been reloaded has none — and getting that wrong shows a green dot beside a
 * server that is switched off.
 */
export type ServerState = {
  tone: "ok" | "error" | "off" | "unknown";
  label: string;
};

export function stateOf(view: McpServerView): ServerState {
  if (view.disabled) return { tone: "off", label: "off" };

  const status = view.status;
  if (!status) return { tone: "unknown", label: "not connected yet" };
  if (status.connected) {
    return {
      tone: "ok",
      label: `${status.tool_count} ${status.tool_count === 1 ? "tool" : "tools"}`,
    };
  }
  return { tone: "error", label: status.error ?? "failed to start" };
}

/**
 * Whether this entry can be saved, and what to say if not.
 *
 * The backend checks all of this again and is the authority — this is here so
 * the answer arrives while the field is still focused rather than after a round
 * trip. The name rules are the backend's for a reason worth restating: the name
 * becomes part of every tool name as `mcp__<server>__<tool>`, so a space or a
 * second double underscore produces tools the model cannot call.
 */
export function draftProblem(draft: McpServerDraft): string | null {
  const name = draft.name.trim();
  if (!name) return "Give the server a name.";
  if (name.includes("__"))
    return "A name cannot contain a double underscore — tool names are built as mcp__<server>__<tool>.";
  if (!/^[A-Za-z0-9_-]+$/.test(name))
    return "Use letters, digits, hyphens, or underscores: the name becomes part of every tool name.";

  if (draft.transport === "stdio") {
    if (!draft.command.trim()) return "Give the command that starts the server.";
  } else {
    const url = draft.url.trim();
    if (!url) return "Give the server's URL.";
    if (!/^https?:\/\//.test(url) && !url.includes("${"))
      return "The URL has to start with http:// or https://.";
  }

  const values = draft.transport === "stdio" ? draft.env : draft.headers;
  const named = values.filter((v) => v.key.trim());
  if (new Set(named.map((v) => v.key.trim())).size !== named.length)
    return "Two entries have the same name.";

  return null;
}

/** A fresh row for the env/headers editor. */
export function blankValue(): McpValue {
  return { key: "", value: "", secret: false };
}

/**
 * Whether this transport uses `env` or `headers`.
 *
 * The two are the same control over the same idea — names and values the server
 * needs — and the only difference is which key they are written under.
 */
export function valuesFor(transport: McpTransport): "env" | "headers" {
  return transport === "stdio" ? "env" : "headers";
}


/**
 * What a catalogue entry still needs before it can be installed.
 *
 * Required inputs only. An optional one left blank is a decision, not an
 * omission — the argument it fills is deleted rather than written empty, which
 * is what `fill` does below.
 */
export function missingInputs(
  entry: CatalogEntry,
  answers: Record<string, string>,
): string[] {
  return entry.inputs
    .filter((input) => input.required && !(answers[input.key] ?? "").trim())
    .map((input) => input.label);
}

/**
 * Puts the answers into one templated string.
 *
 * Returns null when a placeholder in it has no answer, which is how an optional
 * input removes the argument it belongs to rather than leaving `{timezone}` or
 * an empty `--local-timezone=` on the command line. Both of those are worse
 * than the argument being absent: the server either takes the literal brace as
 * a value or refuses to start, and neither failure points back here.
 */
function fill(text: string, answers: Record<string, string>): string | null {
  let missing = false;
  const out = text.replace(/\{([A-Za-z0-9_-]+)\}/g, (whole, key: string) => {
    const value = answers[key];
    if (value === undefined || value.trim() === "") {
      missing = true;
      return whole;
    }
    return value;
  });
  return missing ? null : out;
}

/** The same, for a list of `env` or `headers` pairs. */
function fillValues(
  pairs: { key: string; value: string }[],
  answers: Record<string, string>,
): McpValue[] {
  return pairs.flatMap((pair) => {
    const value = fill(pair.value, answers);
    // A header whose token was left blank is dropped rather than sent empty:
    // `Authorization: Bearer` is a request that fails with a confusing 401,
    // where no header at all fails with the one the server means.
    return value === null ? [] : [{ key: pair.key, value, secret: false }];
  });
}

/**
 * A catalogue entry and its answers, as an entry the form can save.
 *
 * Done here rather than in Rust so there is exactly one thing that decides what
 * a form produces: an installed server and one typed by hand go through the
 * same `McpServerDraft`, the same validation, and the same Test. The catalogue
 * supplies the knowledge — which package, which argument, which header — and
 * stops there.
 */
export function fromCatalog(
  entry: CatalogEntry,
  answers: Record<string, string>,
  scope: Scope,
): McpServerDraft {
  const stdio = entry.transport === "stdio";
  return {
    name: entry.id,
    scope,
    transport: stdio ? "stdio" : "http",
    command: stdio ? (fill(entry.command, answers) ?? entry.command) : "",
    args: stdio
      ? entry.args.flatMap((arg) => {
          const filled = fill(arg, answers);
          return filled === null ? [] : [filled];
        })
      : [],
    env: stdio ? fillValues(entry.env, answers) : [],
    url: stdio ? "" : (fill(entry.url, answers) ?? entry.url),
    headers: stdio ? [] : fillValues(entry.headers, answers),
    disabled: false,
  };
}

/**
 * Whether installing this entry at this scope would write a credential into a
 * file inside the workspace.
 *
 * The one thing the catalogue can catch that a person filling in a form cannot.
 * A project-scope entry lands in `<workspace>/.taurus/mcp.json`, which is a file
 * in somebody's repository — and the commit that leaks the token is one
 * `git add .` away. Every shipped entry that wants a secret defaults to the
 * global file; this is what happens when the scope is changed by hand.
 */
export function leaksSecret(entry: CatalogEntry, scope: Scope): boolean {
  return scope === "workspace" && entry.inputs.some((i) => i.kind === "secret");
}

/**
 * Which config layer a catalogue entry wants to be written into.
 *
 * The catalogue says `project` where the harness says `workspace`; the two
 * names are the same layer, and the mapping lives here rather than in the
 * catalogue so that crate keeps no dependency on the one above it.
 */
export function scopeFor(entry: CatalogEntry): Scope {
  return entry.scope === "project" ? "workspace" : "global";
}

/**
 * The catalogue, narrowed to what somebody typed.
 *
 * Name, blurb and keywords, because what gets typed is rarely the name — "db"
 * for Postgres, "folder" for Filesystem. Blocked entries match too, and that is
 * the point of them: searching for the thing you cannot have should return the
 * reason rather than nothing.
 */
export function searchCatalog(
  entries: CatalogEntry[],
  query: string,
): CatalogEntry[] {
  const wanted = query.trim().toLowerCase();
  if (!wanted) return entries;
  return entries.filter((entry) =>
    [entry.name, entry.blurb, entry.id, ...entry.keywords]
      .join(" ")
      .toLowerCase()
      .includes(wanted),
  );
}
