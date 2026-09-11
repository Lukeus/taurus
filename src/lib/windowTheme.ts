import { useSyncExternalStore } from "react";

/**
 * The window's theme, as it is now and as it changes.
 *
 * Read off `data-theme` on the root element, which `lib/theme` keeps set to
 * `light` or `dark` whatever the preference is — so this follows a switch made
 * in Settings, and a system appearance change, without being told about either.
 *
 * For the two things drawn by a library that has its own idea of a theme: the
 * sketch editor, and a sketch drawn into a note. Everything else in the app
 * reads its colours out of the tokens and never needs to ask.
 */
export function useWindowTheme(): "light" | "dark" {
  return useSyncExternalStore(
    (notify) => {
      const watch = new MutationObserver(notify);
      watch.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["data-theme"],
      });
      return () => watch.disconnect();
    },
    () => (document.documentElement.dataset.theme === "light" ? "light" : "dark"),
    () => "dark",
  );
}

/**
 * Something that changes whenever the window's colours do: the mode, or any
 * token a custom theme paints over it.
 *
 * For the terminal, which is handed its colours rather than reading them out of
 * the stylesheet. The mode alone misses a custom theme swapped for another in
 * the same mode; the preference misses the system turning dark at dusk under
 * "system". Both land on the root element, as `data-theme` and as the tokens in
 * its `style`, so watching the two misses neither.
 */
export function useWindowPalette(): string {
  return useSyncExternalStore(
    (notify) => {
      const watch = new MutationObserver(notify);
      watch.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["data-theme", "style"],
      });
      return () => watch.disconnect();
    },
    () => {
      const root = document.documentElement;
      return `${root.dataset.theme ?? ""}|${root.style.cssText}`;
    },
    () => "",
  );
}
