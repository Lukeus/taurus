import { createContext } from "react";

import type { LineRange } from "../bindings/LineRange";

/**
 * What a link in rendered Markdown points at.
 *
 * Three kinds, because a link in this app can go three places: out to the
 * browser, into the canvas, or down the page it's on. Everything with a scheme
 * is the browser's, since the opener already knows which schemes it allows and
 * says so when it refuses one. Everything else is a path, which in a coding
 * window is a file in the workspace — `docs/retries.md` from a README, or
 * `src/App.tsx#L42-L51` from an answer pointing at code.
 */
export type Target =
  | { kind: "web"; href: string }
  | { kind: "file"; path: string; lines: LineRange | null }
  | { kind: "heading"; slug: string };

/**
 * Which kind `href` is, and where it goes.
 *
 * `base` is the folder a relative path is read from: the open file's own, in
 * the canvas's preview, and the workspace's root everywhere else. A path that
 * starts with the workspace is made relative to it; any other path starting
 * with `/` is read from the root, the way a repository's own links are. Lines
 * come from a `#L42` or `#L42-L51` fragment, the form editors and code hosts
 * write, or from a `:42` or `:42-51` suffix, the form compilers print.
 */
export function classify(href: string, base: string, workspace: string | null): Target | null {
  const raw = href.trim();
  if (!raw) return null;
  if (raw.startsWith("#")) {
    const slug = decode(raw.slice(1));
    return slug ? { kind: "heading", slug: slug.toLowerCase() } : null;
  }
  let rest = raw;
  if (/^file:/i.test(rest)) rest = rest.replace(/^file:(\/\/[^/]*)?/i, "");
  else if (/^[a-z][a-z0-9+.-]*:/i.test(rest)) return { kind: "web", href: raw };

  const hash = rest.indexOf("#");
  const fragment = hash >= 0 ? rest.slice(hash + 1) : "";
  let path = decode(hash >= 0 ? rest.slice(0, hash) : rest.replace(/\?.*$/, ""));
  if (!path) return null;

  let lines = fragmentLines(fragment);
  if (!lines) {
    const suffix = /^(.*?):(\d+)(?:[-:](\d+))?$/.exec(path);
    if (suffix) {
      path = suffix[1];
      lines = range(Number(suffix[2]), suffix[3] ? Number(suffix[3]) : null);
    }
  }

  const root = workspace?.replace(/\\/g, "/").replace(/\/+$/, "");
  path = path.replace(/\\/g, "/");
  if (root && (path === root || path.startsWith(`${root}/`))) path = path.slice(root.length + 1);
  else if (path.startsWith("/")) path = path.replace(/^\/+/, "");
  else if (base) path = `${base.replace(/\/+$/, "")}/${path}`;
  return { kind: "file", path: tidy(path), lines };
}

/** The folder a file is in, as a base for the links inside it. */
export function folderOf(path: string): string {
  const at = path.replace(/\\/g, "/").lastIndexOf("/");
  return at < 0 ? "" : path.slice(0, at);
}

/**
 * The anchor a heading gets, the way GitHub makes them: lower case, spaces to
 * hyphens, punctuation dropped. What a README's table of contents links to.
 */
export function slug(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s_-]/gu, "")
    .replace(/\s/g, "-");
}

/**
 * Opens a workspace file in the canvas. Provided by `App`; absent in a test or
 * anywhere with no canvas to open into, where a path link goes to the opener
 * as it always did and gets told why it can't.
 */
export const FileLinks = createContext<
  ((path: string, lines: LineRange | null) => void) | null
>(null);

/** The folder relative links are read from. The workspace root unless said. */
export const LinkBase = createContext("");

function fragmentLines(fragment: string): LineRange | null {
  const m = /^L(\d+)(?:-L?(\d+))?$/i.exec(fragment);
  return m ? range(Number(m[1]), m[2] ? Number(m[2]) : null) : null;
}

function range(from: number, to: number | null): LineRange | null {
  if (!(from >= 1)) return null;
  const end = to !== null && to >= from ? to : from;
  return { from, to: end };
}

/** `.` and `..` folded away. A `..` past the root stays, for the host to refuse. */
function tidy(path: string): string {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === ".." && parts.length > 0 && parts[parts.length - 1] !== "..") parts.pop();
    else parts.push(part);
  }
  return parts.join("/");
}

function decode(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}
