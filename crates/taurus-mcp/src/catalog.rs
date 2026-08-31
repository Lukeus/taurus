//! The servers the panel offers to add, and the ones it explains instead.
//!
//! # Why a list in the repository rather than a registry
//!
//! [`crate::draft`] refuses to let the *model* install a server, and the
//! argument there governs this file too: the reviewable artifact for
//! `npx -y @scope/package` is a package name, which says nothing about what the
//! package does. A catalogue is the one arrangement that answers it, because
//! the review happens somewhere a person can do it properly — in a commit, once,
//! against the source — rather than in a dialog at the moment somebody wants the
//! feature. Every entry here names its homepage for the same reason: a link to
//! the code is more than a package name, and it is the least a list like this
//! owes the person reading it.
//!
//! It goes stale, and that is the honest cost. Nothing here is fetched, so a
//! package that moves between releases is wrong until somebody bumps this file.
//! What that cannot do is break a working setup: installing copies the entry
//! into `mcp.json` and the catalogue never looks at it again, so a server you
//! added last month goes on running exactly as it was.
//!
//! # What "install" means
//!
//! Writing one entry into `mcp.json`. Nothing is downloaded and no installer
//! runs — `npx` and `uvx` fetch the program at launch, the way they already do
//! for an entry typed by hand. The blast radius of pressing the button is a
//! config file.
//!
//! # Blocked entries
//!
//! Half of what people search for cannot be offered, and saying why is worth
//! more than an empty result. Postgres has no first-party server since the
//! reference one was withdrawn over a SQL-injection vulnerability; Drive, Linear
//! and the rest are hosted behind OAuth, which this client does not speak. An
//! entry with [`CatalogEntry::blocked`] set carries the reason and no command,
//! and the panel draws it as an explanation rather than as something to press.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The catalogue as it ships, parsed once.
const SOURCE: &str = include_str!("../catalog/servers.json");

/// One name/value pair in a template, before its placeholders are filled.
#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct CatalogValue {
    pub key: String,
    /// May contain `{input}` placeholders naming entries in [`CatalogEntry::inputs`].
    pub value: String,
}

/// What kind of box an input gets, and how its value is treated.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum InputKind {
    Text,
    /// A filesystem path. Drawn with a picker beside the box.
    Path,
    /// A credential. Never echoed back out of the config layer once written —
    /// see `McpValue` on the host side for the rule this feeds into.
    Secret,
}

/// Something the person adding this server has to supply.
///
/// The reason the catalogue is more than a list of command lines. An entry that
/// wrote `npx -y …server-filesystem` and asked for no directory would install a
/// server that refuses every path it is given, and the failure would arrive
/// later, in a tool call, wearing no connection to the button that caused it.
#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct CatalogInput {
    /// The name a `{placeholder}` refers to.
    pub key: String,
    pub label: String,
    /// One or two sentences. Says what the value is *for*, not what it is.
    pub help: String,
    pub kind: InputKind,
    pub required: bool,
    #[serde(default)]
    #[ts(optional)]
    pub placeholder: Option<String>,
    /// Where to go and get it. Only where that is a specific page.
    #[serde(default)]
    #[ts(optional)]
    pub link: Option<String>,
}

/// Which file an entry wants to be written into by default.
///
/// A plain string rather than the host's `Scope`, so this crate keeps no
/// dependency on the layer above it. The panel maps it, and lets it be changed.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum CatalogScope {
    /// This workspace only. For servers whose reach is a particular tree.
    Project,
    /// Every workspace. Also where a credential belongs, since the global file
    /// is not one anybody commits.
    Global,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum CatalogTransport {
    Stdio,
    Http,
}

/// One row of the catalogue.
#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub blurb: String,
    pub homepage: String,
    /// Extra words a search should match. What somebody types is rarely the
    /// name — "db" for Postgres, "folder" for Filesystem.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Set when this cannot be installed, and why. Everything below is absent
    /// or ignored on such an entry.
    #[serde(default)]
    #[ts(optional)]
    pub blocked: Option<String>,
    #[serde(default = "global")]
    pub scope: CatalogScope,
    /// The program that must be on PATH for a stdio entry to start. Checked
    /// before anything is filled in, so "uvx is not installed" arrives before
    /// somebody has typed a connection string rather than after.
    #[serde(default)]
    #[ts(optional)]
    pub requires: Option<String>,
    #[serde(default = "stdio")]
    pub transport: CatalogTransport,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<CatalogValue>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: Vec<CatalogValue>,
    #[serde(default)]
    pub inputs: Vec<CatalogInput>,
}

fn global() -> CatalogScope {
    CatalogScope::Global
}

fn stdio() -> CatalogTransport {
    CatalogTransport::Stdio
}

/// The catalogue, and the day somebody last checked it against upstream.
#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct Catalog {
    /// ISO date. Shown in the panel, because a list of programs to run is worth
    /// knowing the age of.
    pub revised: String,
    pub entries: Vec<CatalogEntry>,
}

/// Parses the shipped catalogue.
///
/// Panics on a malformed file, which is the right failure for something baked
/// into the binary: it cannot be wrong at runtime without having been wrong in
/// the commit, and the test below runs on every build.
pub fn catalog() -> Catalog {
    serde_json::from_str(SOURCE).expect("the shipped catalogue must parse")
}

impl CatalogEntry {
    /// Every `{placeholder}` this entry's template refers to.
    ///
    /// Deliberately a scan of the strings rather than a list maintained beside
    /// them: the two would drift, and the direction they drift in is an entry
    /// that installs with `{token}` sitting in the header, literally.
    pub fn placeholders(&self) -> Vec<String> {
        let mut found = Vec::new();
        let sides = self
            .args
            .iter()
            .chain(std::iter::once(&self.command))
            .chain(std::iter::once(&self.url))
            .chain(self.env.iter().map(|value| &value.value))
            .chain(self.headers.iter().map(|value| &value.value));
        for text in sides {
            let mut rest = text.as_str();
            while let Some(start) = rest.find('{') {
                let Some(end) = rest[start..].find('}') else { break };
                let name = &rest[start + 1..start + end];
                if !name.is_empty() && !found.iter().any(|seen| seen == name) {
                    found.push(name.to_string());
                }
                rest = &rest[start + end + 1..];
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the file being data: it can be wrong, and this is
    /// what stops it shipping wrong. Every one of these has a failure mode that
    /// reaches the user as a server that will not start, wearing no connection
    /// to the entry that caused it.
    #[test]
    fn every_entry_is_well_formed() {
        let catalog = catalog();
        assert!(!catalog.entries.is_empty());

        let mut seen: Vec<&str> = Vec::new();
        for entry in &catalog.entries {
            let id = &entry.id;
            assert!(
                !seen.contains(&id.as_str()),
                "two entries share the id {id}"
            );
            seen.push(id);

            assert!(!entry.name.is_empty(), "{id} has no name");
            assert!(!entry.blurb.is_empty(), "{id} has no blurb");
            // The answer to "a package name says nothing about what it does".
            assert!(
                entry.homepage.starts_with("https://"),
                "{id} must link somewhere a person can read the source"
            );

            if let Some(reason) = &entry.blocked {
                // A blocked entry is an explanation. One carrying a command
                // would be an install button that does nothing.
                assert!(reason.len() > 40, "{id} is blocked without saying why");
                assert!(entry.command.is_empty(), "{id} is blocked but has a command");
                assert!(entry.url.is_empty(), "{id} is blocked but has a url");
                assert!(entry.inputs.is_empty(), "{id} is blocked but asks for input");
                continue;
            }

            match entry.transport {
                CatalogTransport::Stdio => {
                    assert!(!entry.command.is_empty(), "{id} runs nothing");
                    assert!(entry.url.is_empty(), "{id} is stdio and has a url");
                    // Every stdio entry fetches its program at launch, and
                    // which fetcher is the question the PATH panel answers.
                    assert!(
                        entry.requires.is_some(),
                        "{id} must say which program has to be on PATH"
                    );
                }
                CatalogTransport::Http => {
                    assert!(
                        entry.url.starts_with("https://"),
                        "{id} must reach a server over TLS"
                    );
                    assert!(entry.command.is_empty(), "{id} is http and has a command");
                }
            }

            // Both directions. A placeholder with no input is written into the
            // config literally; an input with no placeholder is a box whose
            // value goes nowhere, which is worse — it looks like it worked.
            for name in entry.placeholders() {
                assert!(
                    entry.inputs.iter().any(|input| input.key == name),
                    "{id} uses {{{name}}} and never asks for it"
                );
            }
            for input in &entry.inputs {
                assert!(
                    entry.placeholders().contains(&input.key),
                    "{id} asks for {} and never uses it",
                    input.key
                );
                assert!(!input.label.is_empty(), "{id}/{} has no label", input.key);
                assert!(
                    input.help.len() > 20,
                    "{id}/{} needs help text worth reading",
                    input.key
                );
                if let Some(link) = &input.link {
                    assert!(link.starts_with("https://"), "{id}/{} link", input.key);
                }
            }
        }
    }

    #[test]
    fn a_credential_is_never_offered_into_a_workspace_file() {
        // The footgun this catalogue must not build. A secret written at
        // project scope lands in `<workspace>/.taurus/mcp.json`, which is a
        // file in somebody's repository — and the commit that leaks it is one
        // `git add .` away. The panel warns if the scope is changed by hand;
        // the shipped default must never start there.
        for entry in catalog().entries {
            let secret = entry
                .inputs
                .iter()
                .any(|input| input.kind == InputKind::Secret);
            if secret {
                assert_eq!(
                    entry.scope,
                    CatalogScope::Global,
                    "{} wants a credential and defaults to the workspace file",
                    entry.id
                );
            }
        }
    }

    #[test]
    fn placeholders_are_found_wherever_they_are_written() {
        let entry: CatalogEntry = serde_json::from_str(
            r#"{
                "id": "x", "name": "X", "blurb": "b", "homepage": "https://x",
                "transport": "http", "url": "https://x/{region}/mcp",
                "headers": [{ "key": "Authorization", "value": "Bearer {token}" }],
                "env": [{ "key": "K", "value": "{token}" }],
                "inputs": []
            }"#,
        )
        .unwrap();
        // Found in the url as readily as in a header — a scan that only looked
        // at the obvious side would write the other one out literally — and
        // deduplicated, since `token` appears twice.
        assert_eq!(entry.placeholders(), ["region", "token"]);
    }

    #[test]
    fn the_catalogue_says_when_it_was_last_checked() {
        // A bundled list of programs to run is worth knowing the age of, and
        // the panel shows this. A missing date would parse and read as new.
        let revised = catalog().revised;
        assert_eq!(revised.len(), 10, "an ISO date: {revised}");
        assert!(revised.starts_with("20"), "{revised}");
    }
}
