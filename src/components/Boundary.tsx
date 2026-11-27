import { Component, type ErrorInfo, type ReactNode } from "react";

/**
 * Where a render that throws stops.
 *
 * Without one, React unmounts the whole tree on an uncaught render error, so a
 * diagram whose source the parser chokes on, or a table card handed a shape it
 * did not expect, took the rail, the transcript and the box you were typing in
 * with it — a blank window and nothing to do but quit. With one, the part that
 * broke says so in its own place and everything around it keeps working.
 *
 * `label` names the part, in the words the fallback uses ("This step", "The
 * terminal"), because "Something went wrong" with no subject leaves the reader
 * to guess which of the things on screen it means.
 *
 * `resetKey` clears a caught error when it changes — a different file in the
 * canvas, a different conversation — so a failure does not outlive the thing
 * that caused it. "Try again" does the same by hand, for a failure that was a
 * moment rather than a shape (a race on first paint, a module still loading).
 */
export class Boundary extends Component<
  {
    label: string;
    children: ReactNode;
    resetKey?: unknown;
    /** Fill the window rather than sit inline. Only the root uses this. */
    whole?: boolean;
  },
  { error: Error | null; key: unknown }
> {
  state: { error: Error | null; key: unknown } = { error: null, key: this.props.resetKey };

  static getDerivedStateFromError(error: unknown) {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  static getDerivedStateFromProps(
    props: { resetKey?: unknown },
    state: { error: Error | null; key: unknown },
  ) {
    return Object.is(props.resetKey, state.key) ? null : { error: null, key: props.resetKey };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    // The console is the only place the stack survives to; the fallback shows
    // the message, which is what a reader can act on.
    console.error(`[${this.props.label}]`, error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    const { label, whole } = this.props;
    const retry = () => this.setState({ error: null });
    if (whole) {
      return (
        <div role="alert" className="boundary whole">
          <h1 className="m-0 font-display text-24 font-semibold">Taurus hit a problem it could not draw</h1>
          <p className="m-0 max-w-measure text-14 text-dim">
            Your conversations and files are safe: nothing is lost when the window fails to
            render. Reloading the window usually clears it.
          </p>
          <code className="boundary-detail">{error.message}</code>
          <div className="flex gap-2">
            <button className="primary" onClick={() => window.location.reload()}>
              Reload the window
            </button>
            <button onClick={() => void navigator.clipboard?.writeText(detail(error))}>
              Copy details
            </button>
          </div>
        </div>
      );
    }
    return (
      <div role="alert" className="boundary">
        <span className="boundary-dot" aria-hidden="true" />
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="text-13 text-ink">{label} could not be shown.</span>
          <code className="boundary-detail">{error.message}</code>
        </div>
        <button className="quiet" onClick={retry}>
          Try again
        </button>
      </div>
    );
  }
}

function detail(error: Error): string {
  return `${error.name}: ${error.message}\n\n${error.stack ?? ""}`;
}
