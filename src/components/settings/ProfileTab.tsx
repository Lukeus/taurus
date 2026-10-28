// The Profile tab: who is using Taurus, and whether the model is told.

import { useEffect, useState } from "react";

import * as api from "../../lib/api";
import type { Profile, ProfileView } from "../../lib/api";
import { useStore } from "../../state/store";
import { Problem } from "../Problem";
import { Field } from "./Field";

/**
 * The active profile.
 *
 * Read when the tab opens rather than carried on the status, because nothing
 * else on screen draws it and the status is pushed after every turn.
 *
 * The name commits on blur or Enter, like the fields on Terminal, and the
 * switch commits as it's flipped. Each save sends the whole profile, so the
 * two can't write over each other with stale halves.
 */
export function ProfileTab() {
  const status = useStore((s) => s.status);
  const [view, setView] = useState<ProfileView | null>(null);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .getProfile()
      .then((loaded) => {
        if (!live) return;
        setView(loaded);
        setName(loaded.profile.name);
      })
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, []);

  const saved = view?.profile ?? { name: "", introduce: false };

  const save = async (next: Profile) => {
    setError(null);
    try {
      const written = await api.saveProfile(next);
      setView(written);
      setName(written.profile.name);
    } catch (e) {
      // The name is left as typed, so the one that was refused is the one on
      // screen beside the reason.
      setError(String(e));
    }
  };

  const commitName = () => {
    if (name.trim() === saved.name) return setName(saved.name);
    void save({ ...saved, name });
  };

  // What the file said that couldn't be used. Shown here because this is the
  // tab a save from fixes it, which is what `ProblemSource::where_to_fix` says.
  const problems = (status?.problems ?? []).filter((p) => p.source === "profile");

  return (
    <>
      <Field
        label="Your name"
        hint="What you'd like to be called. It stays on this computer unless you turn on the switch below."
      >
        <input
          aria-label="Your name"
          placeholder="no name"
          autoComplete="name"
          value={name}
          disabled={view === null}
          onChange={(e) => setName(e.target.value)}
          onBlur={commitName}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
          }}
        />
      </Field>

      <label className="settings-check">
        <input
          type="checkbox"
          checked={saved.introduce && saved.name !== ""}
          // Nothing to introduce without a name, and a switch that turns on and
          // does nothing would read as broken.
          disabled={view === null || saved.name === ""}
          onChange={(e) => void save({ ...saved, introduce: e.target.checked })}
        />
        <span>
          Tell the model your name
          <span className="hint">
            {saved.name === ""
              ? "Add a name first."
              : "Adds one line to every conversation's instructions, sent to whichever provider you're using. Takes effect on your next message."}
          </span>
        </span>
      </label>

      {view && <p className="hint">Kept in {view.path}.</p>}

      {problems.map((p) => (
        <Problem key={p.message}>{p.message}</Problem>
      ))}
      {error && <Problem>{error}</Problem>}
    </>
  );
}
