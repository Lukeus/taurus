import { useEffect, useMemo, useState } from "react";

import * as api from "../lib/api";
import type { CatalogEntry, McpServerView } from "../lib/api";
import { searchCatalog } from "../lib/mcp";
import { Problem } from "./Problem";

/**
 * The servers Taurus knows how to add, and the ones it explains instead.
 *
 * A list reviewed in a commit rather than a registry search, for the reason
 * `taurus_mcp::catalog` sets out at length: the reviewable artifact for
 * `npx -y @scope/package` is a package name, and a search box handing back
 * package names would be asking for a decision nobody in the loop has the
 * information to make. What a curated list can do, and a search cannot, is link
 * the source and say what each server actually needs before you commit to it.
 *
 * Half of what people type has no answer — Postgres has had no first-party
 * server since the reference one was withdrawn over a SQL-injection
 * vulnerability, and Drive, Linear and the rest are hosted behind OAuth, which
 * this client does not speak. Those are entries too. Searching for the thing
 * you cannot have returns the reason rather than nothing, which is the whole
 * argument for carrying them.
 *
 * Rendered inside the MCP panel rather than over it. Browsing is a step on the
 * way to adding a server, not an errand of its own, and the list of what is
 * already installed is the context that makes it read properly.
 */
export function McpCatalog({
  installed,
  onPick,
  onBack,
}: {
  installed: McpServerView[];
  onPick: (entry: CatalogEntry) => void;
  onBack: () => void;
}) {
  const [catalog, setCatalog] = useState<api.Catalog | null>(null);
  /*
   * Which fetchers are actually on the PATH this application inherited, or null
   * before the answer arrives.
   *
   * The question behind almost every stdio failure, asked before anything is
   * filled in rather than after the server refuses to start. `uvx` being
   * installed and `uvx` being *reachable* are different facts: a window opened
   * from the Dock inherits the launcher's PATH and not a shell's, which is what
   * the PATH section further down the panel exists to explain.
   */
  const [onPath, setOnPath] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");

  useEffect(() => {
    api
      .mcpCatalog()
      .then((found) => {
        setCatalog(found);
        // Asked for what this catalogue actually names, rather than a fixed
        // pair — an entry added later needing `docker` gets its warning for
        // free.
        const wanted = [
          ...new Set(
            found.entries.flatMap((entry) =>
              entry.blocked || !entry.requires ? [] : [entry.requires],
            ),
          ),
        ];
        return api.programsOnPath(wanted).then(setOnPath);
      })
      .catch((e) => setError(String(e)));
  }, []);

  const shown = useMemo(
    () => searchCatalog(catalog?.entries ?? [], query),
    [catalog, query],
  );

  return (
    <>
      <div className="drawer-head">
        <button className="link" onClick={onBack}>
          ← Back
        </button>
        <h2>Add a server</h2>
        <div className="spacer" />
        {catalog && (
          <span
            className="micro"
            data-tip="Shipped with this version rather than fetched, so it never waits on a network — and goes out of date between releases"
          >
            checked {catalog.revised}
          </span>
        )}
      </div>

      <p className="drawer-intro">
        Servers Taurus knows the setup for. Adding one writes an entry into
        <code> mcp.json</code> — nothing is downloaded, and the program is
        fetched at launch the same way an entry typed by hand is.
      </p>

      <input
        className="catalog-search"
        value={query}
        autoFocus
        placeholder="Search — postgres, github, files…"
        onChange={(e) => setQuery(e.target.value)}
        aria-label="Search the catalogue"
      />

      {error && <Problem>{error}</Problem>}
      {!catalog && !error && <p className="drawer-loading">Reading the list…</p>}

      {catalog && shown.length === 0 && (
        <p className="drawer-empty">
          Nothing here matches that. Any server can still be added by hand —
          go back and use <b>Add by hand</b> with the command from its README.
        </p>
      )}

      <ul className="card-list">
        {shown.map((entry) => (
          <CatalogCard
            key={entry.id}
            entry={entry}
            // Matched on the name the install would write, which is the
            // entry's id — so a second Filesystem cannot be added under a name
            // that silently replaces the first.
            //
            // Never on a blocked entry, even where something of that name is
            // configured. "Added" beside "there is no server here to
            // recommend" reads as a contradiction, and the paragraph is the
            // only thing on that card worth reading.
            already={
              !entry.blocked &&
              installed.some((server) => server.name === entry.id)
            }
            reachable={
              // Unknown until the answer lands, which reads better as no
              // warning at all than as a warning that might be wrong.
              entry.requires == null || onPath === null
                ? true
                : onPath.includes(entry.requires)
            }
            onPick={() => onPick(entry)}
          />
        ))}
      </ul>
    </>
  );
}

function CatalogCard({
  entry,
  already,
  reachable,
  onPick,
}: {
  entry: CatalogEntry;
  already: boolean;
  reachable: boolean;
  onPick: () => void;
}) {
  return (
    <li className={`card catalog-card${entry.blocked ? " blocked" : ""}`}>
      <div className="card-body">
        <div className="card-row">
          <span className="card-title">{entry.name}</span>
          {entry.transport === "http" && !entry.blocked && (
            <span className="tag" data-tip="Runs on the vendor's machines; nothing is installed here">
              hosted
            </span>
          )}
          {already && <span className="tag">added</span>}
          <div className="spacer" />
          {/* The answer to "a package name says nothing about what the program
              does". It is the least a list like this owes the person reading
              it, so it sits on every card including the blocked ones. */}
          <a
            className="link"
            href={entry.homepage}
            target="_blank"
            rel="noreferrer"
          >
            Source
          </a>
        </div>

        <p className="catalog-blurb">{entry.blurb}</p>

        {entry.blocked ? (
          // An explanation, not a disabled button. A greyed-out Install would
          // say "not yet"; these are mostly "not ever, by this route".
          <p className="catalog-blocked">{entry.blocked}</p>
        ) : (
          <div className="card-row">
            {!reachable && (
              <span
                className="tag warn"
                data-tip={`Taurus cannot see ${entry.requires} on its PATH — the server will not start until it can. See the PATH section in the panel behind this.`}
              >
                needs {entry.requires}
              </span>
            )}
            <div className="spacer" />
            <button className="primary" onClick={onPick}>
              {already ? "Add another" : "Add"}
            </button>
          </div>
        )}
      </div>
    </li>
  );
}
