import { type ReactNode } from "react";

/**
 * Why something did not work, in the one line it gets.
 *
 * Twenty places said this, in eleven panels, all of them spelling it
 * `className="settings-problem"` — a name that was true when Settings was the
 * only screen that could fail and had been wrong for a long time. It is in the
 * MCP drawer, the agent editor, the trace panel, the changes drawer: every
 * surface that reads something off disk and can come back empty-handed.
 *
 * The name mattered more than a name usually does. Deleting these rules during
 * the Settings conversion, on the strength of the prefix, took the styling off
 * all eleven at once and nothing failed — the sentence saying a save had been
 * refused simply stopped being red. A shape this widely shared should not be
 * findable only by knowing which drawer happened to write it first.
 *
 * A `<p>` in every case, including the two that used to be a `<div>`: the
 * margin is zeroed either way, so they rendered identically, and one of the two
 * was a paragraph of prose wearing the wrong tag.
 */
export function Problem({ children }: { children: ReactNode }) {
  return <p className="problem m-0 text-12-5 text-danger">{children}</p>;
}

/**
 * A run of them under a label, for a read that came back with a list.
 *
 * Renders nothing when the list is empty, which is what every caller was
 * already writing as `{problems.length > 0 && (…)}` — a guard four of the five
 * had and the fifth would have needed the moment its list could be empty.
 *
 * The label is a parameter because one of them is not "Could not load": the MCP
 * drawer says "Could not read", and it is right to. Those servers are files
 * this app parsed rather than a catalogue it assembled, and the difference is
 * the difference between "we could not build the list" and "this line of your
 * config does not make sense".
 */
export function Problems({
  label = "Could not load",
  problems,
}: {
  label?: string;
  /** The messages, already unwrapped from whatever carried them. */
  problems: readonly string[];
}) {
  if (problems.length === 0) return null;
  return (
    <section className="section">
      <span className="micro">{label}</span>
      {problems.map((problem) => (
        <Problem key={problem}>{problem}</Problem>
      ))}
    </section>
  );
}
