// The form pieces every settings tab is built from.

import { useStore } from "../../state/store";

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="settings-field">
      <span className="micro">{label}</span>
      {children}
      {hint && <span className="hint">{hint}</span>}
    </label>
  );
}

/** An empty box means "unset", not an empty string. */
export function blank(value: string): string | null {
  return value.trim() === "" ? null : value;
}

/**
 * Whether two config values are the same setting.
 *
 * `models` is a list, so `!==` on it reports an override for every provider
 * that has one — two equal arrays are never the same object. Compared by
 * content instead; everything else is a scalar and compares as one.
 */
export function same(a: unknown, b: unknown): boolean {
  if (Array.isArray(a) || Array.isArray(b)) {
    return JSON.stringify(a ?? []) === JSON.stringify(b ?? []);
  }
  return (a ?? null) === (b ?? null);
}

/**
 * Says so when the open project's own settings file sets this field.
 *
 * Settings writes the global file, and a project's value wins over it, so a
 * change made here can't show in this workspace: the switch or number moves
 * back to the project's. Without this that read as the write not taking.
 */
export function FromProject({ field }: { field: string }) {
  const status = useStore((s) => s.status);
  if (!status?.settings.from_project.includes(field)) return null;
  return (
    <p className="hint">
      This project's <code>.taurus/settings.json</code> sets this, so that's
      what is used here. A change here applies to your other workspaces.
    </p>
  );
}
