import { describe, expect, it } from "vitest";

import { hooksFileIn } from "./hooksFile";

describe("the hooks file a message names", () => {
  const home = "/Users/me/.taurus/hooks.json";

  it.each([
    // What `taurus_hooks::config` writes, on its own and through `merge`.
    [`${home} doesn't parse: trailing comma at line 1. Its hooks can't run, so prompts and tool calls are refused until it's fixed.`, home],
    [`hook 'hooks.json' ${home} doesn't parse: x`, home],
    [`hook 'guard' has no \`command\` (in ${home})`, home],
    // A refused prompt, however the error is wrapped.
    [`Refused by hook 'broken hooks.json (user_prompt_submit)': ${home} has no \`hooks\` key at its top level.`, home],
    ["/My Projects/app/.taurus/hooks.json isn't the right shape: x", "/My Projects/app/.taurus/hooks.json"],
    ["C:\\Users\\me\\.taurus\\hooks.json could not be read: x", "C:\\Users\\me\\.taurus\\hooks.json"],
    ["\\\\?\\C:\\w\\.taurus\\hooks.json could not be read: x", "\\\\?\\C:\\w\\.taurus\\hooks.json"],
  ])("finds it in: %s", (message, file) => {
    expect(hooksFileIn(message)).toBe(file);
  });

  it.each([
    "/plugins/lint/hooks/hooks.json doesn't parse",
    "The provider stopped answering.",
    "see .taurus/hooks.json for the format",
    "/x/.taurus/hooks.json.bak is odd",
  ])("offers nothing for: %s", (message) => {
    expect(hooksFileIn(message)).toBeNull();
  });
});
