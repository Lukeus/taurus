/**
 * Reports every CSP refusal back to `capture.mjs`, which fails the run on one.
 *
 * Imported before anything else in `main.tsx`, so it is listening before the
 * app loads a font, a chunk or an image. A refusal is otherwise silent — a font
 * draws in a fallback face, an image draws as nothing — and a picture of the
 * fallback looks like a picture. Posted to the page's own origin, which the
 * policy allows; served by anything else, the post goes nowhere and harms
 * nothing.
 */
document.addEventListener("securitypolicyviolation", (event) => {
  const from = event.sourceFile ? ` (from ${event.sourceFile}:${event.lineNumber})` : "";
  void fetch("/__csp-violation", {
    method: "POST",
    body: `${event.effectiveDirective} refused ${event.blockedURI || "an inline source"}${from}`,
  }).catch(() => {});
});
