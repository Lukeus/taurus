// A stand-in for drawing text, for tests of what's said about missing fonts.
// jsdom has no canvas, so without one every font reads as unknown.
import { vi } from "vitest";

/**
 * A canvas whose text widths come from which family draws them: one that's
 * "installed" has its own width, and anything else falls through to the
 * generic family after it, as a browser's does.
 */
export function fakeFonts(have: string[], { sameAs }: { sameAs?: Record<string, string> } = {}) {
  const widths: Record<string, number> = { monospace: 600, "sans-serif": 500, serif: 450 };
  const measure = vi.fn();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(function () {
    const context = {
      font: "",
      measureText(text: string) {
        measure(context.font, text);
        const families = context.font.replace(/^\d+px /, "").split(/,\s*/);
        for (const raw of families) {
          const family = raw.replace(/^"(.*)"$/, "$1");
          if (family in widths) return { width: widths[family] };
          if (have.includes(family)) {
            const like = sameAs?.[family];
            return { width: like ? widths[like] : 700 };
          }
        }
        return { width: 0 };
      },
    };
    return context as unknown as CanvasRenderingContext2D;
  } as unknown as typeof HTMLCanvasElement.prototype.getContext);
  return measure;
}
