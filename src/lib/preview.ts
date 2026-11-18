/**
 * The model's look at a preview, as the window takes it.
 *
 * The host asks (`preview-look`), `App` picks the tab and brings it to the
 * front, and the tab's own looker settles the width and says what it shows.
 * Rust takes the picture from the rectangle the looker reports. See
 * `src-tauri/src/preview.rs`.
 */
import type { BackgroundJob } from "../bindings/BackgroundJob";
import type { Logged } from "../bindings/Logged";
import type { LookRequest } from "../bindings/LookRequest";

/** What a tab says it showed. The page's words in it are the page's. */
export type LookAnswer = {
  url: string;
  title: string;
  /** As the model is told it: `phone (390px)`. */
  viewport: string;
  /** Where the page is, in this page's CSS pixels, for the picture. */
  area: { x: number; y: number; width: number; height: number; viewport: number };
  outline: string | null;
  console: Logged[];
};

export type Looker = (request: LookRequest) => Promise<LookAnswer>;

const lookers = new Map<string, Looker>();

/** A tab offering itself to be looked at, by its path. Returns the undo. */
export function offerLook(path: string, looker: Looker): () => void {
  lookers.set(path, looker);
  return () => {
    if (lookers.get(path) === looker) lookers.delete(path);
  };
}

export const lookerFor = (path: string) => lookers.get(path);

/**
 * Whether a hostname is this machine, the way the preview means it:
 * `localhost` or `127.0.0.1`. Not `[::1]`, which the window's content policy
 * can't name; Rust reports an IPv6 server as `localhost`. The frame script in
 * `preview.rs` and `loopback` in the tools crate say the same.
 */
export const isLoopbackHost = (hostname: string) => hostname === "localhost" || hostname === "127.0.0.1";

/** An address's `host:port`, or the address itself if it isn't one. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/**
 * What makes two addresses the same server: a loopback name, the scheme and
 * the port. `localhost` and `127.0.0.1` both reach it. `null` for an address
 * that isn't on this machine.
 */
export function loopbackServer(url: string): string | null {
  try {
    const parsed = new URL(url);
    if (!isLoopbackHost(parsed.hostname)) return null;
    const port = parsed.port || (parsed.protocol === "https:" ? "443" : "80");
    return `${parsed.protocol}//:${port}`;
  } catch {
    return null;
  }
}

/** Whether a running command is serving `url`. */
export function servedBy(jobs: BackgroundJob[], url: string): boolean {
  const server = loopbackServer(url);
  return (
    server !== null &&
    jobs.some((job) => job.running && job.url && loopbackServer(job.url) === server)
  );
}

export const wait = (ms: number) => new Promise<void>((done) => setTimeout(done, ms));

/** After the browser has drawn `n` more frames. */
export const frames = (n: number) =>
  new Promise<void>((done) => {
    const step = (left: number) =>
      left <= 0 ? done() : requestAnimationFrame(() => step(left - 1));
    step(n);
  });
