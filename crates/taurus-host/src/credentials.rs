//! Saved credentials: tokens kept in the keychain and named from `mcp.json`.
//!
//! # Why
//!
//! An MCP server that wants a personal access token used to get it one of two
//! ways: typed into the panel, where it was written into `mcp.json` in plain
//! text, or exported into a shell and named as `${VAR}`, which the panel never
//! mentioned and an app launched from the Dock doesn't inherit. A credential
//! saved here is a third way: the value goes to the OS keychain, the file says
//! `${secret:NAME}`, and the file is safe to read, share and commit.
//!
//! # Where one may go
//!
//! A reference is only text, and a project's `.taurus/mcp.json` can write it
//! as easily as yours can. So each credential lists the hosts it's for, and
//! [`ProfileCredentials::reveal`] holds every use to these rules:
//!
//! - **An HTTP server** gets it when its URL's host is on the list. A list
//!   left empty means anywhere, and only your own `mcp.json` may use one of
//!   those.
//! - **A program** started over stdio gets it in its environment only when
//!   your own `mcp.json` defines it. A project's program is a program the
//!   project chose, and it could send the value anywhere.
//!
//! Workspace trust already stands between a project and its servers. This is
//! the second gate, for the case trust can't see: a folder you trusted once,
//! and a `git pull` since that added a server.
//!
//! # Where it's kept
//!
//! The list (names and hosts, never values) is
//! `~/.taurus/profiles/<id>/credentials.json`, beside the profile it belongs
//! to, so a second profile gets credentials of its own. The values are in the
//! keychain under [`keychain_key`]. The default profile's keys are
//! `cred:<name>`. Any other profile's carry its id as well, so adding one
//! never renames what's already stored.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use taurus_tools::env::check_secret_name;
use taurus_tools::{Credentials, Destination, Scope};
use ts_rs::TS;

use crate::{config, profile, secrets};

/// The file the list is kept in, inside a profile's directory.
const CREDENTIALS_FILE: &str = "credentials.json";

/// One saved credential, as the list keeps it. The value isn't here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Credential {
    /// The hosts it may be sent to over HTTP. `*.example.com` covers every
    /// subdomain of `example.com` but not `example.com` itself. Empty is
    /// anywhere, from your own `mcp.json` only.
    #[serde(default)]
    pub hosts: Vec<String>,
}

/// One saved credential, as Settings › Profile shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CredentialView {
    pub name: String,
    pub hosts: Vec<String>,
    /// Whether the keychain holds a value for it. Asked without reading it,
    /// so drawing the list never raises a keychain dialog.
    pub stored: bool,
    /// The configured servers whose entries name it.
    pub used_by: Vec<String>,
}

/// Where a credential's value is kept in the keychain.
pub fn keychain_key(profile_id: &str, name: &str) -> String {
    if profile_id == profile::DEFAULT_ID {
        format!("cred:{name}")
    } else {
        format!("profile:{profile_id}:cred:{name}")
    }
}

fn file(profile_id: &str) -> PathBuf {
    profile::dir(profile_id).join(CREDENTIALS_FILE)
}

/// The list. A missing file is an empty one.
pub fn load(profile_id: &str) -> Result<BTreeMap<String, Credential>, String> {
    read(&file(profile_id))
}

fn read(path: &Path) -> Result<BTreeMap<String, Credential>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(&text).map_err(|e| format!("{} isn't valid: {e}", path.display()))
}

fn write(profile_id: &str, list: &BTreeMap<String, Credential>) -> Result<(), String> {
    let path = file(profile_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("couldn't create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(list)
        .map_err(|e| format!("couldn't write the credential list: {e}"))?;
    config::replace_file(&path, &json).map_err(|e| format!("couldn't save {}: {e}", path.display()))
}

/// Reads the list for an edit, refusing a file that won't parse, for the
/// reason [`config::edit_settings`] does.
fn load_for_edit(profile_id: &str) -> Result<BTreeMap<String, Credential>, String> {
    load(profile_id).map_err(|e| format!("Fix it or remove it first: {e}"))
}

/// Adds a credential or changes one.
///
/// `value` is `None` to change only the hosts, which is refused for a
/// credential that doesn't exist yet: a name with nothing behind it fails at
/// connect time with a message about the keychain, which is a worse place to
/// find out. The value goes to the keychain before the list is written, so a
/// keychain that refuses leaves the list as it was.
pub fn save(
    profile_id: &str,
    name: &str,
    hosts: &[String],
    value: Option<&str>,
) -> Result<CredentialView, String> {
    let name = name.trim();
    check_secret_name(name).map_err(capitalized)?;
    let hosts = tidy_hosts(hosts)?;
    let mut list = load_for_edit(profile_id)?;
    let key = keychain_key(profile_id, name);
    match value.map(str::trim) {
        Some(value) if !value.is_empty() => secrets::store(&key, value)?,
        _ if list.contains_key(name) => {}
        _ => return Err("Paste the token to save it.".into()),
    }
    list.insert(
        name.to_string(),
        Credential {
            hosts: hosts.clone(),
        },
    );
    write(profile_id, &list)?;
    Ok(CredentialView {
        name: name.to_string(),
        hosts,
        stored: secrets::exists(&key),
        used_by: Vec::new(),
    })
}

/// Removes a credential, value first, so a keychain that refuses leaves it
/// listed rather than leaving a value nothing lists.
pub fn remove(profile_id: &str, name: &str) -> Result<(), String> {
    let mut list = load_for_edit(profile_id)?;
    secrets::clear(&keychain_key(profile_id, name))?;
    if list.remove(name).is_some() {
        write(profile_id, &list)?;
    }
    Ok(())
}

/// The list as Settings shows it, with which servers use each.
pub fn views(
    profile_id: &str,
    servers: &BTreeMap<String, taurus_mcp::ServerConfig>,
) -> Result<Vec<CredentialView>, String> {
    Ok(load(profile_id)?
        .into_iter()
        .map(|(name, credential)| CredentialView {
            stored: secrets::exists(&keychain_key(profile_id, &name)),
            used_by: servers
                .iter()
                .filter(|(_, server)| server.uses_secret(&name))
                .map(|(server, _)| server.clone())
                .collect(),
            hosts: credential.hosts,
            name,
        })
        .collect())
}

/// Hosts as the list keeps them: lowercase, deduplicated, and a pasted URL
/// cut down to its host, since copying the server's URL is the obvious way to
/// fill the box in.
fn tidy_hosts(hosts: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in hosts {
        let raw = raw.trim().to_ascii_lowercase();
        if raw.is_empty() {
            continue;
        }
        let host = if raw.contains("://") {
            url::Url::parse(&raw)
                .ok()
                .and_then(|url| url.host_str().map(str::to_string))
                .ok_or_else(|| format!("{raw} isn't a URL with a host in it."))?
        } else {
            raw
        };
        let bare = host.strip_prefix("*.").unwrap_or(&host);
        if bare.is_empty()
            || bare.contains('*')
            || !bare
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '[' | ']'))
        {
            return Err(format!(
                "{host} isn't a host. Write one like mcp.example.com, or *.example.com for \
                 every subdomain."
            ));
        }
        if !out.contains(&host) {
            out.push(host);
        }
    }
    Ok(out)
}

/// Whether `host` is one a credential listing `allowed` may go to.
fn allowed(allowed: &[String], host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    allowed.iter().any(|entry| match entry.strip_prefix("*.") {
        Some(domain) => host
            .strip_suffix(domain)
            .is_some_and(|sub| sub.ends_with('.') && sub.len() > 1),
        None => *entry == host,
    })
}

fn capitalized(text: String) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

/// The active profile's credentials, read from the keychain when a server
/// starts and held to the rules in the module docs.
pub struct ProfileCredentials;

impl ProfileCredentials {
    /// Whether `name` may go to `to`, before anything is read.
    fn check(name: &str, credential: &Credential, to: &Destination<'_>) -> Result<(), String> {
        let ours = to.layer == Scope::Global;
        match to.host {
            Some(host) if !credential.hosts.is_empty() => {
                if allowed(&credential.hosts, host) {
                    Ok(())
                } else {
                    Err(format!(
                        "`{name}` is only sent to {}, and this server is at {host}. Add {host} \
                         to it in Settings › Profile if that's where it belongs.",
                        credential.hosts.join(", ")
                    ))
                }
            }
            Some(host) if !ours => Err(format!(
                "`{name}` isn't limited to any host, so only your own mcp.json can use it, and \
                 this server comes from the project. Add {host} to it in Settings › Profile to \
                 let this server have it."
            )),
            Some(_) => Ok(()),
            None if !ours => Err(format!(
                "this server is a program the project chose, so it can't have `{name}`. Define \
                 it in your own mcp.json instead, or give it an environment variable."
            )),
            None => Ok(()),
        }
    }
}

impl Credentials for ProfileCredentials {
    fn reveal(&self, name: &str, to: &Destination<'_>) -> Result<String, String> {
        let id = profile::active_id();
        // Read per use rather than held: this runs when a server starts, which
        // is rare, and a list held since the last reload would let a host just
        // taken off it keep receiving the token until something reloaded.
        let list = load(id)?;
        let credential = list.get(name).ok_or_else(|| {
            format!("there's no saved credential called `{name}`. Add it in Settings › Profile.")
        })?;
        Self::check(name, credential, to)?;
        secrets::stored(&keychain_key(id, name)).ok_or_else(|| {
            format!(
                "`{name}` is listed but the keychain has nothing for it. Paste it again in \
                 Settings › Profile."
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::isolated_home;

    fn to(layer: Scope, host: Option<&str>) -> Destination<'_> {
        Destination { layer, host }
    }

    fn hosts(list: &[&str]) -> Vec<String> {
        list.iter().map(|h| h.to_string()).collect()
    }

    /// Unique per test: the test keychain is process-wide, and so is the
    /// default profile's key for a name.
    fn name(test: &str) -> String {
        format!("credentials-test-{test}")
    }

    #[test]
    fn a_saved_credential_comes_back_where_it_may_go() {
        let _home = isolated_home();
        let n = name("roundtrip");
        save(
            profile::DEFAULT_ID,
            &n,
            &hosts(&["mcp.linear.app"]),
            Some(" lin_123 "),
        )
        .unwrap();

        let value = ProfileCredentials
            .reveal(&n, &to(Scope::Workspace, Some("mcp.linear.app")))
            .unwrap();
        assert_eq!(value, "lin_123", "trimmed, as a paste usually needs");
        // The list holds the hosts and never the value.
        let text = std::fs::read_to_string(file(profile::DEFAULT_ID)).unwrap();
        assert!(
            text.contains("mcp.linear.app") && !text.contains("lin_123"),
            "{text}"
        );
    }

    #[test]
    fn a_listed_host_is_the_only_place_it_goes_whoever_asks() {
        let credential = Credential {
            hosts: hosts(&["mcp.linear.app"]),
        };
        let err = ProfileCredentials::check(
            "linear",
            &credential,
            &to(Scope::Global, Some("evil.example")),
        )
        .unwrap_err();
        assert!(
            err.contains("evil.example") && err.contains("mcp.linear.app"),
            "{err}"
        );
    }

    #[test]
    fn a_credential_for_anywhere_is_only_for_your_own_file() {
        let anywhere = Credential::default();
        assert!(ProfileCredentials::check("t", &anywhere, &to(Scope::Global, Some("a.b"))).is_ok());
        let err = ProfileCredentials::check("t", &anywhere, &to(Scope::Workspace, Some("a.b")))
            .unwrap_err();
        assert!(err.contains("Add a.b"), "{err}");
    }

    #[test]
    fn a_projects_program_never_gets_one() {
        // Not even one with hosts listed: a program can send it anywhere.
        let listed = Credential {
            hosts: hosts(&["api.github.com"]),
        };
        assert!(ProfileCredentials::check("t", &listed, &to(Scope::Workspace, None)).is_err());
        assert!(ProfileCredentials::check("t", &listed, &to(Scope::Global, None)).is_ok());
    }

    #[test]
    fn a_wildcard_covers_subdomains_and_not_the_domain_or_a_lookalike() {
        let list = hosts(&["*.example.com"]);
        assert!(allowed(&list, "mcp.example.com"));
        assert!(allowed(&list, "A.B.Example.com"));
        assert!(!allowed(&list, "example.com"));
        assert!(!allowed(&list, "evilexample.com"));
    }

    #[test]
    fn a_pasted_url_is_kept_as_its_host() {
        assert_eq!(
            tidy_hosts(&hosts(&[
                "https://MCP.Linear.app/sse",
                "mcp.linear.app",
                " "
            ]))
            .unwrap(),
            hosts(&["mcp.linear.app"])
        );
        assert!(tidy_hosts(&hosts(&["not a host"])).is_err());
        assert!(tidy_hosts(&hosts(&["*.*.com"])).is_err());
    }

    #[test]
    fn a_new_credential_needs_a_value_and_an_existing_one_keeps_its_own() {
        let _home = isolated_home();
        let n = name("keep");
        let err = save(profile::DEFAULT_ID, &n, &[], None).unwrap_err();
        assert!(err.contains("Paste"), "{err}");

        save(profile::DEFAULT_ID, &n, &[], Some("first")).unwrap();
        save(profile::DEFAULT_ID, &n, &hosts(&["a.example"]), None).unwrap();
        assert_eq!(
            ProfileCredentials
                .reveal(&n, &to(Scope::Global, Some("a.example")))
                .unwrap(),
            "first"
        );
    }

    #[test]
    fn a_name_that_will_not_read_the_same_everywhere_is_refused() {
        let _home = isolated_home();
        let err = save(profile::DEFAULT_ID, "my token", &[], Some("x")).unwrap_err();
        assert!(err.starts_with("A credential's name"), "{err}");
    }

    #[test]
    fn removing_one_takes_the_value_and_the_listing_together() {
        let _home = isolated_home();
        let n = name("remove");
        save(profile::DEFAULT_ID, &n, &[], Some("v")).unwrap();
        remove(profile::DEFAULT_ID, &n).unwrap();
        assert!(load(profile::DEFAULT_ID).unwrap().is_empty());
        assert!(!secrets::exists(&keychain_key(profile::DEFAULT_ID, &n)));
        let err = ProfileCredentials
            .reveal(&n, &to(Scope::Global, None))
            .unwrap_err();
        assert!(err.contains("no saved credential"), "{err}");
    }

    #[test]
    fn listing_says_who_uses_each_without_reading_any() {
        let _home = isolated_home();
        let n = name("views");
        save(profile::DEFAULT_ID, &n, &[], Some("v")).unwrap();
        let before = secrets::reads(&keychain_key(profile::DEFAULT_ID, &n));
        let servers = BTreeMap::from([(
            "linear".to_string(),
            taurus_mcp::ServerConfig::Http {
                url: "https://mcp.linear.app".into(),
                headers: BTreeMap::from([(
                    "Authorization".to_string(),
                    format!("Bearer ${{secret:{n}}}"),
                )]),
                disabled: false,
            },
        )]);
        let views = views(profile::DEFAULT_ID, &servers).unwrap();
        assert_eq!(views[0].used_by, vec!["linear".to_string()]);
        assert!(views[0].stored);
        assert_eq!(
            secrets::reads(&keychain_key(profile::DEFAULT_ID, &n)),
            before,
            "drawing the list read the keychain"
        );
    }

    #[test]
    fn another_profile_keeps_its_values_under_its_own_names() {
        assert_eq!(keychain_key(profile::DEFAULT_ID, "github"), "cred:github");
        assert_eq!(keychain_key("work", "github"), "profile:work:cred:github");
    }
}
