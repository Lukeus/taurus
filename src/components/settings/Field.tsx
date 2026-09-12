// The form pieces every settings tab is built from.



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
