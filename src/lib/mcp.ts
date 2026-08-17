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
