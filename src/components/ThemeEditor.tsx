import { useEffect, useLayoutEffect, useMemo, useState } from "react";

import * as api from "../lib/api";
import type { CustomTheme, Scope, ThemeFile } from "../lib/api";
import { failures, parseHex, type Result } from "../lib/contrast";
import {
  COLOR_GROUPS,
  livePalette,
  previewTheme,
  slug,
  type ResolvedTheme,
} from "../lib/theme";
import { Modal } from "./Modal";

/**
 * The editor behind "Customize" in Settings › Appearance.
 *
 * Three things shape it.
 *
 * It previews live, on the real window rather than in a swatch. A palette is
 * not a set of colours, it is how those colours look under each other at the
 * sizes this app actually uses them — and no preview tile the size of a
 * postcard can tell you that the accent you picked is invisible on a 10px
 * mono label in the rail. So the whole window repaints as the picker moves,
 * and Cancel puts it back.
 *
 * It edits one mode at a time, and switching which repaints the window. The
 * alternative — two columns of fourteen swatches — is twenty-eight controls
 * where half are for a palette you cannot see, which is how a light theme
 * ends up shipped untested by the person who wrote its dark half.
 *
 * And it checks contrast against the *resolved* palette, not the draft. A
 * theme that names four colours inherits ten, and the pairs that matter are
 * almost always one of each: an accent measured only against the ink the
 * theme states would be measured against nothing at all.
 */
export function ThemeEditor({
  /** The theme being edited, or null to start a new one from what is showing. */
  editing,
  /** The mode the app is in, which is the one the editor opens on. */
  mode,
  onClose,
  onSaved,
}: {
  editing: CustomTheme | null;
  mode: ResolvedTheme;
  onClose: () => void;
  onSaved: (id: string) => void;
}) {
  const [draft, setDraft] = useState<Draft>(() => start(editing, mode));
  /** Which palette the fields below are editing, and the window is showing. */
  const [painting, setPainting] = useState<ResolvedTheme>(mode);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  /**
   * The stylesheet's own palette, per mode, captured with no theme on it.
   *
   * The base a draft resolves against, and it has to be the *stylesheet's*
   * rather than whatever is painting right now: applying a theme clears every
   * token before it sets its own, so a colour this draft does not name will
   * come out as the shipped one, not as the previous theme's. Captured per
   * mode because the two shipped palettes are different fourteen colours.
   */
  const [bases, setBases] = useState<Partial<Record<ResolvedTheme, Record<string, string>>>>(
    {},
  );

  /*
   * The draft, on the window, as it is typed.
   *
   * Not cached — see `previewTheme`. A draft that was remembered would be
   * replayed on the next cold start as though it had been saved, so backing
   * out of an edit would still change how the app looks tomorrow.
   *
   * A layout effect rather than an ordinary one, and both steps in the same
   * pass. The capture has to paint nothing to read the stylesheet underneath,
   * and doing that after the browser had already painted would show a frame of
   * the unthemed palette every time the mode switched.
   */
  useLayoutEffect(() => {
    if (!bases[painting]) {
      previewTheme(painting, null);
      const bare = livePalette();
      setBases((b) => ({ ...b, [painting]: bare }));
    }
    previewTheme(painting, asTheme(draft));
  }, [draft, painting, bases]);

  /*
   * And put it back on the way out, whichever way out was taken — Save,
   * Cancel, Escape, or a click on the scrim. Restoring in each handler
   * instead left Escape painting somebody's abandoned draft until the next
   * status arrived.
   *
   * A Save has already written the file by the time this runs, so the status
   * that lands a moment later repaints the same thing this does. The gap
   * between the two is what this covers.
   */
  useEffect(() => {
    return () => {
      previewTheme(mode, editing);
    };
    // Deliberately empty: this is the state the editor was opened over, and
    // it must not be recaptured as the draft moves.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /**
   * What the palette will actually resolve to: the stylesheet's, with this
   * draft's over the top.
   *
   * Computed rather than read back off the window, which is the fix for a bug
   * the first version had. Reading the document during render happens *before*
   * the effect that paints the draft, so the warnings described the previous
   * render — on the first one, they described no palette at all and the panel
   * stayed empty however unreadable the colours were.
   *
   * Resolving against the base rather than checking only what the theme states
   * is the other half: a theme that names four colours inherits ten, and the
   * pairs that matter are almost always one of each.
   */
  const resolved = useMemo(
    () => ({ ...(bases[painting] ?? {}), ...draft[painting] }),
    [bases, painting, draft],
  );
  const wrong = failures(resolved);

  const set = (patch: Partial<Draft>) => setDraft((d) => ({ ...d, ...patch }));
  const setColor = (key: string, value: string | null) => {
    const palette = { ...draft[painting] };
    if (value === null) delete palette[key];
    else palette[key] = value;
    set({ [painting]: palette } as Partial<Draft>);
  };

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const id = editing?.id ?? slug(draft.name);
      // Back where it came from. A theme a repository ships stays in the
      // repository when it is edited, rather than being quietly forked into
      // the user's home directory where the project can never see it again.
      const scope: Scope = editing?.scope ?? "global";
      await api.saveTheme(scope, id, asFile(draft));
      onSaved(id);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const named = draft.name.trim().length > 0;

  return (
    <Modal onClose={onClose}>
      <aside className="drawer theme-editor" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-head">
          <h2>{editing ? `Edit ${editing.name}` : "New theme"}</h2>
          <button className="drawer-close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>

        <p className="drawer-intro">
          Saved as{" "}
          <code>
            {editing?.path ?? `~/.taurus/themes/${slug(draft.name) || "…"}.json`}
          </code>
          . The window is showing this draft; closing without saving puts it back.
        </p>

        <label className="settings-field">
          <span className="micro">Name</span>
          <input
            value={draft.name}
            onChange={(e) => set({ name: e.target.value })}
            placeholder="Midnight"
          />
        </label>

        {/* Which palette the fields below edit. It repaints the window with
            them, because a light palette edited while the window is dark is a
            light palette nobody has looked at. */}
        <div className="pill-row" role="radiogroup" aria-label="Palette being edited">
          {(["dark", "light"] as const).map((which) => (
            <button
              key={which}
              role="radio"
              aria-checked={painting === which}
              className={`pill${painting === which ? " on" : ""}`}
              onClick={() => setPainting(which)}
            >
              {which === "dark" ? "Dark palette" : "Light palette"}
            </button>
          ))}
          <span className="theme-count">
            {Object.keys(draft[painting]).length} of 14 set
          </span>
        </div>

        {COLOR_GROUPS.map((group) => (
          <section className="section" key={group.label}>
            <span className="micro">{group.label}</span>
            <p className="hint">{group.hint}</p>
            <div className="theme-swatches">
              {group.keys.map((key) => (
                <Swatch
                  key={key}
                  name={key}
                  /* The value the window is painting, which is the theme's own
                     where it set one and the stylesheet's where it did not. */
                  resolved={resolved[key] ?? "#000000"}
                  own={draft[painting][key] ?? null}
                  onChange={(value) => setColor(key, value)}
                />
              ))}
            </div>
          </section>
        ))}

        {wrong.length > 0 && <Contrast failures={wrong} />}

        <section className="section">
          <span className="micro">Typefaces</span>
          <p className="hint">
            Families installed on this machine. Anything not installed falls back
            to the stack the app ships, and no theme can bring a font with it —
            the window loads no remote stylesheets.
          </p>
          {(
            [
              ["display", "Display", "Headings and the wordmark"],
              ["body", "Body", "Everything that is a sentence"],
              ["mono", "Mono", "Code, paths, counts, and every micro-label"],
            ] as const
          ).map(([key, label, hint]) => (
            <label className="settings-field" key={key}>
              <span className="micro">{label}</span>
              <input
                value={draft.fonts[key]}
                placeholder="as shipped"
                onChange={(e) => set({ fonts: { ...draft.fonts, [key]: e.target.value } })}
              />
              <span className="hint">{hint}</span>
            </label>
          ))}
        </section>

        <section className="section">
          <span className="micro">Brand</span>
          <label className="settings-check">
            <input
              type="checkbox"
              checked={draft.wordmark !== null}
              onChange={(e) => set({ wordmark: e.target.checked ? "" : null })}
            />
            <span>
              Replace the wordmark
              <span className="hint">
                Leave the box below empty for a mark on its own.
              </span>
            </span>
          </label>
          {draft.wordmark !== null && (
            <label className="settings-field">
              <span className="micro">Wordmark</span>
              <input
                value={draft.wordmark}
                onChange={(e) => set({ wordmark: e.target.value })}
                placeholder="(mark only)"
              />
            </label>
          )}
          <label className="settings-field">
            <span className="micro">Logo</span>
            <input
              value={draft.logo}
              onChange={(e) => set({ logo: e.target.value })}
              placeholder="mark.svg"
            />
            <span className="hint">
              An SVG or PNG, up to 256KB. A bare name is read from the folder the
              theme file is in, so a logo committed beside it travels with it.
            </span>
          </label>
        </section>

        <section className="section">
          <span className="micro">Shape</span>
          <label className="settings-field">
            <span className="micro">Corner radius</span>
            <input
              type="range"
              min={0}
              max={2}
              step={0.1}
              value={draft.shape.radius === "" ? 1 : Number(draft.shape.radius)}
              onChange={(e) => set({ shape: { ...draft.shape, radius: e.target.value } })}
            />
            <span className="hint">
              {draft.shape.radius === "" || Number(draft.shape.radius) === 1
                ? "As shipped."
                : Number(draft.shape.radius) === 0
                  ? "Square."
                  : `${Number(draft.shape.radius).toFixed(1)}× the shipped ladder.`}
            </span>
          </label>
          {(
            [
              ["gutter", "Centre gutter", "Where the transcript and the composer start"],
              ["railGutter", "Rail gutter", "Where the rail's text starts"],
            ] as const
          ).map(([key, label, hint]) => (
            <label className="settings-field" key={key}>
              <span className="micro">{label}</span>
              <input
                inputMode="numeric"
                value={draft.shape[key]}
                placeholder="as shipped"
                onChange={(e) => set({ shape: { ...draft.shape, [key]: e.target.value } })}
              />
              <span className="hint">{hint}</span>
            </label>
          ))}
        </section>

        {error && <p className="settings-problem">{error}</p>}

        <div className="settings-actions">
          <button className="primary" disabled={!named || saving} onClick={save}>
            {saving ? "Saving…" : "Save theme"}
          </button>
          <button onClick={onClose}>Cancel</button>
          {!named && <span className="hint">A theme needs a name to be saved under.</span>}
        </div>
      </aside>
    </Modal>
  );
}

/**
 * One colour, as the window is painting it and as the theme states it.
 *
 * The two are different facts and the row shows both: the picker is always on
 * the resolved value, so it opens where the eye expects, and whether this
 * theme has an opinion is the presence of the clear button beside it. Without
 * that distinction there is no way to say "inherit" — every swatch would look
 * set, and a theme would grow all fourteen colours the moment it was opened.
 */
function Swatch({
  name,
  resolved,
  own,
  onChange,
}: {
  name: string;
  resolved: string;
  own: string | null;
  onChange: (value: string | null) => void;
}) {
  return (
    <div className={`theme-swatch${own ? " set" : ""}`}>
      <input
        type="color"
        aria-label={name}
        /* `<input type="color">` accepts `#rrggbb` and nothing else, so a
           three-digit hex from a hand-written theme has to be expanded on the
           way in or the control silently falls back to black. */
        value={sixDigit(own ?? resolved)}
        onChange={(e) => onChange(e.target.value)}
      />
      <span className="theme-swatch-name">{name}</span>
      <input
        className="theme-swatch-hex"
        aria-label={`${name} hex`}
        value={own ?? ""}
        placeholder={resolved}
        onChange={(e) => onChange(e.target.value.trim() === "" ? null : e.target.value)}
      />
      <button
        className="theme-swatch-clear"
        disabled={!own}
        aria-label={`Use the built-in ${name}`}
        data-tip="Back to the colour the app ships"
        onClick={() => onChange(null)}
      >
        ✕
      </button>
    </div>
  );
}

/**
 * The pairs that came out unreadable, and where each one is on screen.
 *
 * A warning rather than a block. The floors are WCAG's and they are the right
 * default, but this is somebody's own machine and a theme is not a public
 * website — refusing to save a 4.2:1 would be enforcing a standard on a person
 * who can see their own screen. What it must not do is let it happen silently,
 * which is the state a branding feature arrives in if nobody builds this.
 */
function Contrast({ failures }: { failures: Result[] }) {
  const text = failures.filter((f) => f.kind === "text");
  return (
    <section className="section theme-contrast">
      <span className="micro warn">
        {text.length > 0
          ? `${text.length} of these will be hard to read`
          : "One edge will be hard to see"}
      </span>
      <ul>
        {failures.map((f) => (
          <li key={`${f.fg}-${f.bg}`}>
            <b>{f.what}</b>
            <span>
              {f.fg} on {f.bg} — {f.ratio?.toFixed(1) ?? "?"}:1, needs {f.needs}:1
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

/**
 * What the editor holds while it is open.
 *
 * Strings throughout, including the numbers. A `number | null` here means a
 * half-typed "1" in the gutter box clears the value on its way to "16", and
 * the field fights whoever is typing in it; the parse happens once, on save.
 * `wordmark` is the one three-state field — null is "no opinion", "" is a
 * mark standing on its own — and it is a checkbox in the UI for that reason.
 */
type Draft = {
  name: string;
  dark: Record<string, string>;
  light: Record<string, string>;
  fonts: { display: string; body: string; mono: string };
  wordmark: string | null;
  logo: string;
  shape: { radius: string; gutter: string; railGutter: string };
};

/**
 * The draft an editor opens with.
 *
 * A new theme starts from the palette that is *showing* rather than from
 * nothing, so the first thing somebody does is change a colour rather than
 * invent fourteen. It is read off the document — see `livePalette` — which is
 * also what keeps the shipped values from being restated here in a third
 * place that could drift from `styles.css`.
 */
function start(editing: CustomTheme | null, mode: ResolvedTheme): Draft {
  if (editing) {
    return {
      name: editing.name,
      dark: { ...editing.dark },
      light: { ...editing.light },
      fonts: {
        display: editing.fonts.display ?? "",
        body: editing.fonts.body ?? "",
        mono: editing.fonts.mono ?? "",
      },
      wordmark: editing.wordmark,
      logo: "",
      shape: {
        radius: editing.shape.radius?.toString() ?? "",
        gutter: editing.shape.gutter?.toString() ?? "",
        railGutter: editing.shape["rail-gutter"]?.toString() ?? "",
      },
    };
  }
  return {
    name: "",
    dark: mode === "dark" ? livePalette() : {},
    light: mode === "light" ? livePalette() : {},
    fonts: { display: "", body: "", mono: "" },
    wordmark: null,
    logo: "",
    shape: { radius: "", gutter: "", railGutter: "" },
  };
}

/** The draft as something the preview can paint. */
function asTheme(draft: Draft): CustomTheme {
  return {
    id: "",
    name: draft.name,
    path: "",
    scope: "global",
    dark: draft.dark,
    light: draft.light,
    fonts: {
      display: draft.fonts.display || null,
      body: draft.fonts.body || null,
      mono: draft.fonts.mono || null,
    },
    wordmark: draft.wordmark,
    logo: null,
    shape: shapeOf(draft),
    // Both, always. The editor paints whichever mode it is showing, and a
    // draft that pinned one would stop the switch above from working.
    modes: "both",
  };
}

/** The draft as the file that gets written. */
function asFile(draft: Draft): ThemeFile {
  return {
    name: draft.name.trim(),
    dark: draft.dark,
    light: draft.light,
    fonts: {
      display: draft.fonts.display.trim() || null,
      body: draft.fonts.body.trim() || null,
      mono: draft.fonts.mono.trim() || null,
    },
    brand: {
      wordmark: draft.wordmark,
      logo: draft.logo.trim() || null,
    },
    shape: shapeOf(draft),
  };
}

function shapeOf(draft: Draft) {
  const px = (value: string) => {
    const n = Number(value);
    return value.trim() === "" || !Number.isFinite(n) ? null : Math.round(n);
  };
  const radius = Number(draft.shape.radius);
  return {
    // 1 is what the app already does, so storing it would be a key that says
    // nothing — and the file is a thing people read.
    radius:
      draft.shape.radius.trim() === "" || !Number.isFinite(radius) || radius === 1
        ? null
        : radius,
    gutter: px(draft.shape.gutter),
    "rail-gutter": px(draft.shape.railGutter),
  };
}

/** `#abc` as `#aabbcc`, for a control that will not take the short form. */
function sixDigit(value: string): string {
  const rgb = parseHex(value);
  if (!rgb) return "#000000";
  const hex = (n: number) => n.toString(16).padStart(2, "0");
  return `#${hex(rgb.r)}${hex(rgb.g)}${hex(rgb.b)}`;
}
