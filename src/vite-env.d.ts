/**
 * The sliver of Node used by `styles.test.ts`, which reads the stylesheet as
 * text to check it.
 *
 * Declared here rather than by adding `@types/node`: nothing the app ships
 * runs in Node, and one test reading one file is a thin reason to pull the
 * whole standard library into every file's scope. Vite's `?raw` would have
 * avoided this, but Vitest leaves CSS unprocessed by default and returns an
 * empty string for it.
 */
declare module "node:fs" {
  export function readFileSync(path: URL | string, encoding: "utf8"): string;
}
