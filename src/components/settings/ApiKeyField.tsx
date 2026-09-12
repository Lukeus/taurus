// The API key box every provider and search backend shares, and what it says about a stored key.

import { useState } from "react";
import type { KeyStatus } from "../../lib/api";
import { Field } from "./Field";

/** What stands in for the key box where there is nothing to type into. */
export const KEY_NONE = "settings-key-none py-2 text-12-5 text-faint italic";

/**
 * The API key field.
 *
 * Write-only on purpose: the stored key is never sent to the frontend, so there
 * is nothing to prefill and the input starts empty every time. What replaces
 * the reassurance of seeing the old value is [`keyLine`] saying where the key
 * in use is coming from — which is the thing a user actually needs to know, and
 * the thing an obscured field full of dots cannot tell them.
 *
 * The key belongs to a *saved* id. A row that has been renamed or never saved
 * has no id to store against yet, so the field says so rather than failing on
 * the button press.
 *
 * Storing and clearing are passed in rather than chosen here: model providers
 * and search backends keep their keys in the same credential store under
 * different namespaces, and the field is identical either way.
 */
export function ApiKeyField({
  status,
  available,
  onStore,
  onClear,
  onChanged,
  unsavedHint,
  unreadable = null,
}: {
  status: KeyStatus | undefined;
  available: boolean;
  onStore: (key: string) => Promise<void>;
  onClear: () => Promise<void>;
  onChanged: () => void;
  unsavedHint: string;
  /** Why the stored keys could not be listed, when they could not. */
  unreadable?: string | null;
}) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!available) {
    return (
      <Field
        label="API key"
        hint="This machine has no credential store Taurus can use, so the key has to come from the environment variable above."
      >
        <div className={KEY_NONE}>unavailable</div>
      </Field>
    );
  }

  // No status means this provider is not in the saved list: either it was just
  // added, or its id was edited and the old key still belongs to the old id.
  if (!status) {
    // Or the list of stored keys could not be read at all, and then "save it
    // first" sends somebody to fix a provider that is already saved.
    if (unreadable) {
      return (
        <Field label="API key" hint={`Could not read which keys are stored: ${unreadable}`}>
          <div className={KEY_NONE}>unknown</div>
        </Field>
      );
    }
    return (
      <Field label="API key" hint={unsavedHint}>
        <div className={KEY_NONE}>not saved yet</div>
      </Field>
    );
  }

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      setValue("");
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const stored = status.kind === "keychain" || status.kind === "overridden";

  return (
    <Field label="API key" hint={keyHint(status)}>
      <div className="settings-key flex items-center gap-2 [&_input]:flex-1 [&_input]:min-w-0 [&_input]:font-mono [&_button]:flex-none">
        <input
          type="password"
          value={value}
          disabled={busy}
          placeholder={stored ? "replace the stored key" : "paste a key to store it"}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => setValue(e.target.value)}
        />
        <button
          disabled={busy || value.trim() === ""}
          onClick={() => run(() => onStore(value))}
        >
          Store
        </button>
        {stored && (
          <button className="danger" disabled={busy} onClick={() => run(onClear)}>
            Remove
          </button>
        )}
      </div>
      {error && <div className="settings-key-error text-12 text-danger">{error}</div>}
    </Field>
  );
}

/** What the field says beneath itself about where the key comes from. */
export function keyHint(status: KeyStatus): string {
  switch (status.kind) {
    case "missing":
      return "Stored in your OS keychain, never on disk and never in a config file.";
    case "keychain":
      return "Stored in your OS keychain and in use.";
    case "environment":
      return `Currently coming from $${status.variable}. A key stored here would be used only if that variable were unset.`;
    case "overridden":
      return `Stored here, but $${status.variable} is set and wins. Unset it for the stored key to take effect.`;
  }
}
