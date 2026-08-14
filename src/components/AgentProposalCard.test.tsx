import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import type { AgentProposal } from "../lib/api";
import { AgentProposalCard } from "./AgentProposalCard";

const proposal = (patch: Partial<AgentProposal> = {}): AgentProposal => ({
  id: "p1",
  name: "diff-reviewer",
  description: "Reviews a diff for correctness bugs and reports what it found",
  prompt: "You review a diff for correctness bugs only. Report the file and line.",
  tools: ["read_file", "grep"],
  max_iterations: 20,
  rationale: "Explained this from scratch twice today",
  replaces_existing: false,
  ...patch,
});

const render = (p: AgentProposal) =>
  renderToStaticMarkup(<AgentProposalCard proposal={p} onResolve={() => {}} />);

describe("agent proposal card", () => {
  it("shows the system prompt without being asked", () => {
    // An approved agent runs on future turns with these instructions. A
    // collapsed prompt is one nobody reads before approving it.
    const html = render(proposal());
    expect(html).toContain("You review a diff for correctness bugs only.");
  });

  it("names the tools the agent would be scoped to", () => {
    // The only field that decides what it can reach.
    const html = render(proposal());
    expect(html).toContain("read_file, grep");
  });

  it("says so plainly when the agent would inherit every tool", () => {
    // `null` is not a missing value here — it means the widest scope there is,
    // and rendering nothing would make the broadest case the quietest one.
    const html = render(proposal({ tools: null }));
    expect(html).toContain("every tool you have");
  });

  it("says when an agent would replace one that already exists", () => {
    expect(render(proposal({ replaces_existing: true }))).toContain(
      "replaces existing",
    );
    expect(render(proposal())).not.toContain("replaces existing");
  });

  it("offers both destinations and a way out", () => {
    const html = render(proposal());
    expect(html).toContain("this project");
    expect(html).toContain("all projects");
    expect(html).toContain("Discard");
  });

  it("omits the rationale row when the model gave none", () => {
    expect(render(proposal())).toContain("Explained this from scratch twice");
    expect(render(proposal({ rationale: "" }))).not.toContain("Why");
  });
});
