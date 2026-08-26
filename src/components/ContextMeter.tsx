import { useStore } from "../state/store";

/** Where the harness begins summarizing, as a fraction of the window. */
const COMPACTS_AT = 0.8;

/** Below this it is a number nobody needs and one more thing on screen. */
const WORTH_SAYING = 0.5;

/**
 * How full the model's context is, above the box you are about to add to.
 *
 * Quiet until it is worth knowing. Below half a window this says nothing at
 * all; from there it appears and climbs, and at the point the harness starts
 * summarizing it says so — which is where it stops being trivia and becomes
 * the explanation for what the transcript is about to do on its own.
 *
 * The window is named as well as the fraction, and that is the half that
 * earns its place. An OpenAI-compatible backend cannot be asked how much it
 * holds, so the figure being counted against may be the built-in assumption
 * rather than the truth — and a conversation that fills implausibly fast is a
 * misconfiguration you can only recognize if the number it is filling up is
 * on screen.
 *
 * Subscribed here rather than in `App`, for the reason the transcript is: this
 * moves before every request, and the topbar and the rail have no business
 * redrawing that often to say the same thing.
 */
export function ContextMeter() {
  const context = useStore((s) => s.context);
  if (!context || context.window === 0) return null;

  const fraction = context.used / context.window;
  if (fraction < WORTH_SAYING) return null;

  const percent = Math.min(100, Math.round(fraction * 100));
  const summarizing = fraction >= COMPACTS_AT;

  return (
    <div
      className={`context-meter${summarizing ? " full" : ""}`}
      data-tip={`${context.used.toLocaleString()} of ${context.window.toLocaleString()} tokens, the system prompt and tool definitions included`}
    >
      <span className="context-bar">
        {/* Width, not a transform: the bar is a few pixels tall and a scaled
            one blurs its own edge at that size. */}
        <span className="context-fill" style={{ width: `${percent}%` }} />
      </span>
      <span className="micro">
        context {percent}% of {short(context.window)}
        {summarizing && " · older turns are being summarized"}
      </span>
    </div>
  );
}

/** 128000 → `128k`. The exact figure is in the tooltip. */
function short(tokens: number): string {
  if (tokens >= 1_000_000) {
    const millions = tokens / 1_000_000;
    return `${millions % 1 === 0 ? millions : millions.toFixed(1)}M`;
  }
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}k`;
  return String(tokens);
}
