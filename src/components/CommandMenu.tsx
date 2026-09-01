import type { CommandSummary } from "../lib/api";

/**
 * The commands a `/` prefix is currently narrowing to.
 *
 * Exported and pure so the matching rules are testable without a DOM: which
 * command a half-typed name resolves to is the part that has to be right, and
 * it is the part a mounted-component test proves least about.
 *
 * Skills and agents rank against each other on the name alone. The harness
 * settles a name they both hold before the list ever gets here, so a row's kind
 * is something to show, never something to sort by.
 */
export function matches(
  commands: CommandSummary[],
  query: string,
): CommandSummary[] {
  // Both sides lowered, not just the query. A name is whatever its author
  // wrote in the frontmatter — nothing lowercases it on the way in, and skills
  // borrowed from other clients are under no obligation to be kebab-case — so
  // comparing a lowered query against a raw name made every skill with a
  // capital letter in it unreachable the moment you typed anything.
  const typed = query.toLowerCase();
  const lowered = new Map(commands.map((c) => [c, c.name.toLowerCase()]));
  const nameOf = (c: CommandSummary) => lowered.get(c) ?? c.name.toLowerCase();
  return (
    commands
      .filter((c) => nameOf(c).includes(typed))
      // A prefix match is what the user is typing toward; an interior match is
      // a happy accident and belongs below it. `speckit-plan` should win `plan`
      // only once nothing starts with it.
      .sort((a, b) => {
        const byPrefix =
          Number(nameOf(b).startsWith(typed)) -
          Number(nameOf(a).startsWith(typed));
        return byPrefix !== 0 ? byPrefix : a.name.localeCompare(b.name);
      })
  );
}

/**
 * Reads the `/name` being typed, or null when the composer is not in a command.
 *
 * Only ever the first word, and only while it is still the whole message: once
 * there is a space the name is settled and the rest is arguments, so the menu
 * gets out of the way rather than hovering over someone writing a sentence.
 *
 * The character set is the one a name may actually be made of — letters in
 * either case, digits, `-` and `_`. It used to be lowercase only, which meant a
 * capital or an underscore did not narrow the menu, it *closed* it: the query
 * became null, and a list that had been showing every command a keystroke ago
 * vanished with no way to tell that from having matched nothing.
 */
export function commandQuery(text: string): string | null {
  if (!text.startsWith("/")) return null;
  const name = text.slice(1);
  if (/\s/.test(name)) return null;
  return /^[\w-]*$/.test(name) ? name : null;
}

export function CommandMenu({
  commands,
  active,
  onPick,
}: {
  commands: CommandSummary[];
  /** Index of the highlighted row. */
  active: number;
  onPick: (command: CommandSummary) => void;
}) {
  if (commands.length === 0) return null;

  return (
    <ul className="command-menu" role="listbox" aria-label="Commands">
      {commands.map((command, i) => (
        <li key={`${command.kind}/${command.name}`}>
          <button
            type="button"
            role="option"
            aria-selected={i === active}
            className={`command-row${i === active ? " on" : ""}`}
            // The composer keeps focus: a click here must complete the name,
            // not take the caret out of the box the user is typing in.
            onMouseDown={(e) => {
              e.preventDefault();
              onPick(command);
            }}
          >
            <span className="command-name">/{command.name}</span>
            <span className="command-when">{command.when_to_use}</span>
            {/* The two kinds do different things with the rest of the line —
                one runs a procedure here, the other hands the job to an agent
                with its own context — so which it is belongs on the row rather
                than in the sending. Last, so it lands in a column of its own
                and the trigger lines stay a block of text to read down. */}
            <span className={`tag command-kind ${command.kind}`}>
              {command.kind}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}
