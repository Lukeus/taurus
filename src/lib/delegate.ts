/**
 * A delegation's report, for the card that draws it.
 *
 * Live, the report arrives as its own event. A reopened conversation has only
 * the tool result the transcript kept, which is the report rendered as text by
 * `DelegateReport::render` in `crates/taurus-tools/src/delegate.rs`. Its first
 * line is fixed in shape so this can read it back.
 */
import type { DelegateDisposition, DelegateReport } from "./api";

const DISPOSITIONS: readonly DelegateDisposition[] = [
  "done",
  "blocked",
  "failed",
  "cancelled",
  "unreported",
];

const STATUS = /^Status: ([a-z]+)\.(?: It needs (you|the user) to: (.+))?$/;
const FILES = "\n\nFiles changed: ";
const TOOLS = "\n\n[sub-agent used: ";

/**
 * The report a delegation's result text carries, or `undefined` for text that
 * isn't one — a result from before reports existed, or a refused call.
 */
export function reportFromText(text: string): DelegateReport | undefined {
  const newline = text.indexOf("\n");
  const first = newline < 0 ? text : text.slice(0, newline);
  const match = STATUS.exec(first);
  if (!match) return undefined;
  const disposition = match[1] as DelegateDisposition;
  if (!DISPOSITIONS.includes(disposition)) return undefined;

  // The tail is the harness's, not the delegate's: the tool summary is added
  // last, and the files list just before it. Cut from the end so a summary
  // that happens to mention either can't be mistaken for them.
  let body = newline < 0 ? "" : text.slice(newline + 1);
  const tools = body.lastIndexOf(TOOLS);
  if (tools >= 0) body = body.slice(0, tools);
  let files: string[] = [];
  const at = body.lastIndexOf(FILES);
  // The harness's list is always a single final line. One with more text
  // after it is the summary quoting something.
  const tail = at >= 0 ? body.slice(at + FILES.length) : "";
  if (at >= 0 && !tail.includes("\n")) {
    files = tail.split(", ").filter(Boolean);
    body = body.slice(0, at);
  }

  return {
    disposition,
    owner: match[2] === undefined ? undefined : match[2] === "you" ? "parent" : "user",
    needs: match[3],
    summary: body.trim(),
    files,
  };
}

/** The chip's words. */
export function reportLabel(report: DelegateReport): string {
  switch (report.disposition) {
    case "done":
      return "Done";
    case "blocked":
      return report.owner === "user" ? "Needs you" : "Blocked";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Stopped";
    case "unreported":
      return "No report";
  }
}

/** A report the harness delivered from a background delegate. */
export type BackgroundReport = { call: string; agent: string; text: string };

const BACKGROUND = /<background-report call="([^"]*)" agent="([^"]*)">\n([\s\S]*?)\n<\/background-report>/g;

/**
 * Takes the background reports out of a message's text, and returns them with
 * whatever text is left.
 *
 * The harness writes these into the conversation so the model can read them,
 * shaped by `Arrival::render` in `crates/taurus-tools/src/pending.rs`. They
 * aren't anything the user said, so a reopened conversation puts each one
 * back on the card of the call that started it instead of drawing a bubble.
 */
export function takeBackgroundReports(text: string): {
  reports: BackgroundReport[];
  rest: string;
} {
  const reports: BackgroundReport[] = [];
  const rest = text.replace(BACKGROUND, (_, call: string, agent: string, body: string) => {
    reports.push({ call, agent, text: body });
    return "";
  });
  return { reports, rest: reports.length > 0 ? rest.trim() : text };
}
