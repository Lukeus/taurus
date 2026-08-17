/**
 * The rail's line art.
 *
 * These were text glyphs — ✦, ◈, ⇅ — which is the cheapest way to get a mark
 * on screen and the least predictable: each one is a character in whatever font
 * the platform has for it, so the same rail rendered at three different weights
 * on three different machines and nothing lined up with the 13px labels beside
 * it. Drawn here instead, on one 16px grid at one stroke weight, so a row of
 * them reads as a set.
 *
 * Every icon inherits `currentColor` and takes its size from the caller, which
 * is what lets a single definition be a faint rail glyph in one place and the
 * accent-coloured one in another without a second copy.
 */

/** Shared frame. `size` is both dimensions; the grid is always 16. */
function Icon({
  size = 13,
  children,
}: {
  size?: number;
  children: React.ReactNode;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      // Decorative without exception: nothing here is the only thing saying
      // what its control does, so a screen reader that skips all of it loses
      // nothing.
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

/**
 * The app mark: the T on its rounded square.
 *
 * The one icon that is filled rather than drawn, and the one that names its own
 * colours — it is a logo, not a glyph, and a rail that tinted it to match the
 * label beside it would be showing a different logo.
 */
export function Logo({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true" focusable="false">
      <rect width="32" height="32" rx="8" fill="var(--accent)" />
      <path
        d="M8 11h16M16 11v11"
        stroke="var(--on-accent)"
        strokeWidth="2.6"
        strokeLinecap="round"
      />
    </svg>
  );
}

/** Two arrows chasing each other: swap this workspace for another. */
export function SwapIcon({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path
        d="M3 6a5 5 0 0 1 8.5-3.2M13 10a5 5 0 0 1-8.5 3.2"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
      <path
        d="M11 1v3h-3M5 15v-3h3"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Icon>
  );
}

export function TrashIcon({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path
        d="M3 4.5h10M6.5 4.5V2.8h3v1.7M4.5 4.5l.6 8.6a1 1 0 0 0 1 .9h3.8a1 1 0 0 0 1-.9l.6-8.6"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Icon>
  );
}

/** Skills. The same four-point star the skill proposal card is marked with. */
export function SparkIcon({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M8 1l1.6 4.6L14 7l-4.4 1.4L8 13l-1.6-4.6L2 7l4.4-1.4L8 1z" fill="currentColor" />
    </Icon>
  );
}

/**
 * Sub-agents: one node handing work down to two.
 *
 * Deliberately not a second star or a second person-shape — the row reads left
 * to right and Skills already owns the star, so this one has to be legible as a
 * different *kind* of thing at 13px, not a variation on its neighbour. Branching
 * is the one property of delegation that survives being drawn that small.
 */
export function DelegateIcon({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <circle cx="8" cy="2.8" r="1.5" fill="currentColor" />
      <path
        d="M8 4.3v1.7M4.3 6h7.4M4.3 6v2M11.7 6v2"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <rect
        x="2.3"
        y="8"
        width="4"
        height="4"
        rx="1.1"
        stroke="currentColor"
        strokeWidth="1.3"
      />
      <rect
        x="9.7"
        y="8"
        width="4"
        height="4"
        rx="1.1"
        stroke="currentColor"
        strokeWidth="1.3"
      />
    </Icon>
  );
}

/**
 * MCP servers: an outward plug on a lead.
 *
 * The row it joins already carries a star and a branching tree, so this one has
 * to say "something outside this app, connected to it" without being either. A
 * plug is the one shape that reads that way at 13px, and it is the metaphor the
 * protocol's own ecosystem uses — so nobody has to learn it here.
 */
export function PlugIcon({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path
        d="M6 1.6v3.2M10 1.6v3.2"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
      />
      <path
        d="M3.7 4.8h8.6v2.1a4.3 4.3 0 01-4.3 4.3 4.3 4.3 0 01-4.3-4.3V4.8z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <path
        d="M8 11.2v3.2"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
      />
    </Icon>
  );
}

/**
 * Settings, as sliders rather than the design's gear.
 *
 * The gear it replaces was a small circle with eight radial spokes, which at
 * 13px is a sun — the same eight strokes the theme row two lines below it draws
 * for light mode, and no amount of tuning the radii told them apart. The design
 * never rendered that pair together. Sliders read as settings at any size and
 * share nothing with a sun.
 */
export function SlidersIcon({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path
        d="M2 4.5h9M2 8h3M2 11.5h7"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
      />
      <circle cx="12.6" cy="4.5" r="1.5" stroke="currentColor" strokeWidth="1.3" />
      <circle cx="6.6" cy="8" r="1.5" stroke="currentColor" strokeWidth="1.3" />
      <circle cx="10.6" cy="11.5" r="1.5" stroke="currentColor" strokeWidth="1.3" />
    </Icon>
  );
}

export function MoonIcon({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path
        d="M13.5 9.8A5.8 5.8 0 0 1 6.2 2.5a5.8 5.8 0 1 0 7.3 7.3z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
    </Icon>
  );
}

/**
 * A filled disc with short rays, where the design drew an outlined one.
 *
 * Its own rays and the gear's spokes are the same eight strokes on the same
 * eight angles — outlined, the two are the same icon at two radii. They are two
 * rows apart in the rail whenever the theme is light, which the design never
 * had to render. Filling the disc and pulling the rays back off it is what
 * separates them at 13px; the gear keeps the hollow centre.
 */
export function SunIcon({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <circle cx="8" cy="8" r="3.1" fill="currentColor" />
      <path
        d="M8 1.2v1.6M8 13.2v1.6M1.2 8h1.6M13.2 8h1.6M3.3 3.3l1.1 1.1M11.6 11.6l1.1 1.1M12.7 3.3l-1.1 1.1M4.4 11.6l-1.1 1.1"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
      />
    </Icon>
  );
}

/**
 * "Whatever the machine is doing" — a display, because the setting is about the
 * machine rather than about light or dark. The design only drew the two it had;
 * this app's third preference needs a mark that is neither of them.
 */
export function DisplayIcon({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <rect
        x="1.8"
        y="2.8"
        width="12.4"
        height="8.4"
        rx="1.4"
        stroke="currentColor"
        strokeWidth="1.3"
      />
      <path d="M5.5 14h5" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
    </Icon>
  );
}

/**
 * A question the user is being asked.
 *
 * Drawn open rather than inside a circle: the card it heads already has a
 * border and an accent, and a ringed glyph beside a ringed card reads as a
 * badge on a badge.
 */
export function QuestionIcon({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path
        d="M5.4 5.6a2.6 2.6 0 113.9 2.25c-.8.48-1.3 1-1.3 1.9"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
      <circle cx="8" cy="12.6" r="1" fill="currentColor" />
    </Icon>
  );
}
