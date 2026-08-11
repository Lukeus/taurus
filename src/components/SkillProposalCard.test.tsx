import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// The command layer reaches into Tauri internals that do not exist outside the
// webview. These tests never let an effect run, but the module is imported.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve([])),
  Channel: class {},
}));

import type { SkillProposal } from "../lib/api";
import { SkillProposalCard } from "./SkillProposalCard";

const proposal = (patch: Partial<SkillProposal> = {}): SkillProposal => ({
  id: "p1",
  name: "release-notes",
  description: "Assemble release notes from merged pull requests",
  when_to_use: "Preparing release notes for a tagged version",
  body: "1. List merged PRs since the last tag.\n2. Group them by label.",
  scripts: [],
  rationale: "Worked this out the hard way twice",
  replaces_existing: false,
  ...patch,
});

const render = (p: SkillProposal) =>
  renderToStaticMarkup(<SkillProposalCard proposal={p} onResolve={() => {}} />);

describe("skill proposal card", () => {
  it("shows the trigger line, which is the part that decides when it fires", () => {
    // `when_to_use` is the only text the model reads when picking a skill
    // later, so approving without seeing it is approving blind.
    const html = render(proposal());
    expect(html).toContain("Preparing release notes for a tagged version");
    expect(html).toContain("release-notes");
  });

  it("shows the procedure without being asked", () => {
    // An approved skill is future instructions. The card opens with the body
    // visible on purpose — a collapsed procedure is one nobody reads.
    const html = render(proposal());
    expect(html).toContain("List merged PRs since the last tag.");
  });

  it("says when a skill would replace one that already exists", () => {
    const html = render(proposal({ replaces_existing: true }));
    expect(html).toContain("replaces existing");
    expect(render(proposal())).not.toContain("replaces existing");
  });

  it("names a bundled script but keeps its source behind a click", () => {
    // The path and interpreter are the summary; the source is long enough that
    // showing every script inline would bury the procedure. It stays one click
    // away, never further.
    const html = render(
      proposal({
        scripts: [
          {
            path: "collect.sh",
            interpreter: "bash",
            description: "collect PRs",
            content: "#!/bin/bash\necho SECRET_MARKER\n",
          },
        ],
      }),
    );
    expect(html).toContain("collect.sh");
    expect(html).toContain("bash");
    expect(html).not.toContain("SECRET_MARKER");
  });

  it("offers both destinations and neither is preselected as permanent", () => {
    const html = render(proposal());
    expect(html).toContain("this project");
    expect(html).toContain("all projects");
    expect(html).toContain("Discard");
  });

  it("omits the rationale row when the model gave none", () => {
    expect(render(proposal())).toContain("Worked this out the hard way twice");
    expect(render(proposal({ rationale: "" }))).not.toContain("Why");
  });
});
