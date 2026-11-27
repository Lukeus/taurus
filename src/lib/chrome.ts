import { APPLE } from "./keys";

/**
 * Whether the window's own title bar is gone and its buttons float over ours.
 *
 * On macOS the window is drawn with an overlay title bar (`tauri.conf.json`):
 * no grey strip with "Taurus" in it above the app's own header, the way every
 * native Mac app of this shape looks. The cost is that the close, minimise and
 * zoom buttons now sit on top of the rail's first row, and that the window can
 * only be dragged by what the page marks as a drag region — so the page has to
 * know. Windows and Linux keep their platform's frame, and a browser (the
 * screenshot build) has no window buttons at all, so both are left alone.
 */
export const OVERLAY =
  APPLE && typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/**
 * Says so on `<html>` as `data-chrome="overlay"`, which is what the stylesheet
 * reads to clear the buttons, and keeps `data-fullscreen` in step: in full
 * screen macOS hides the buttons along with the menu bar, and a row still
 * stepping around where they were would be a gap with nothing in it.
 *
 * Before the first render, like the theme, so the first frame is already laid
 * out around the buttons rather than drawn under them and then moved.
 */
export function applyChrome(): void {
  if (!OVERLAY) return;
  const root = document.documentElement;
  root.dataset.chrome = "overlay";
  void import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
    const win = getCurrentWindow();
    const sync = async () => {
      const full = await win.isFullscreen().catch(() => false);
      if (full) root.dataset.fullscreen = "";
      else delete root.dataset.fullscreen;
    };
    await sync();
    // Entering and leaving full screen both arrive as a resize.
    await win.onResized(() => void sync());
  });
}
