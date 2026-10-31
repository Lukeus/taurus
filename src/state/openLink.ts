import { openUrl } from "@tauri-apps/plugin-opener";

import { useStore } from "./store";

/**
 * Opens a link in the system's handler, and says so when it can't.
 *
 * The link is often the model's: which schemes get through is the capability's
 * call (http, https, mailto and tel), so a `file:` link or a relative path is
 * refused there. A click that does nothing reads as a broken app, so the refusal
 * goes to the banner.
 */
export function openLink(href: string): void {
  void openUrl(href).catch((e) => {
    useStore.getState().noteError(`Couldn't open ${href}: ${e}`);
  });
}
