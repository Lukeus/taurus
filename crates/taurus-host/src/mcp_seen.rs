//! What each MCP server offered the last time it connected, kept on disk.
//!
//! The MCP panel prices a connected server's tools: what its schemas add to
//! every request. A switched-off server has no tools registered, so there was
//! nothing to price, and the panel said nothing — which answered "what did
//! enabling this cost me" and not "what would enabling it cost", the question
//! asked before flipping the switch. Asking the server means starting it,
//! which is what the switch is off to avoid.
//!
//! So the answer comes from the last time it did run. Every reload writes down
//! what each connected server listed, raw, and a server that isn't connected
//! is priced from that. Raw rather than a number, because what a schema costs
//! depends on whether compact schemas are on, and that can change between the
//! time it was seen and the time it is asked about.
//!
//! A record is only used for the entry it was made from. A server renamed onto
//! another's key, or pointed at a different program, is a different server,
//! and the old figure would be a confident wrong answer. A server's tool list
//! can also change without its entry changing — an `npx` package updating —
//! which is why the figure is always shown with when it was seen.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use taurus_provider::ToolDef;

use crate::config;

/// One server, as it was last seen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    /// Its entry as [`taurus_mcp::ServerConfig::describe`] put it then: the
    /// command line or the URL.
    pub entry: String,
    /// When it was last seen running, to within [`REFRESH_SECS`], in
    /// seconds since the Unix epoch.
    pub at: u64,
    /// Its tools, namespaced, as the server wrote them.
    pub tools: Vec<ToolDef>,
}

/// How stale a record's time may get before an unchanged server's record is
/// written again. Every reload sees every running server, and a folder switch
/// or a save in the MCP panel is a reload, so writing on each would rewrite
/// this file far more often than the minute-level "seen 2h ago" needs.
pub const REFRESH_SECS: u64 = 3600;

impl Seen {
    /// What these tools add to every request, as the registry would advertise
    /// them now: compacted or not, and without the ones `disabled` switches
    /// off, which switching the server on doesn't bring back.
    pub fn tokens(&self, compact: bool, disabled: &[String]) -> u32 {
        self.tools
            .iter()
            .filter(|tool| !disabled.contains(&tool.name))
            .map(|tool| {
                let schema = if compact {
                    taurus_tools::schema::compact(&tool.input_schema)
                } else {
                    tool.input_schema.clone()
                };
                crate::usage::schema_cost(&ToolDef {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: taurus_tools::schema::slim(&schema),
                })
            })
            .sum()
    }
}

fn path() -> PathBuf {
    config::home_dir().join("cache").join("mcp-seen.json")
}

/// Every server on record. Empty when there's no file, or one that won't
/// parse: this is a cache, and the panel without it is the panel as it was.
pub fn load() -> BTreeMap<String, Seen> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Records what these servers offer now, keeping every other server's record.
///
/// `entries` is each server's entry as described now, which is what a later
/// read checks a record against. A server whose tools and entry are unchanged
/// has its time moved on only once it is [`REFRESH_SECS`] old, so an ordinary
/// reload writes nothing.
pub fn record(
    now: BTreeMap<String, Vec<ToolDef>>,
    entries: &BTreeMap<String, String>,
) -> std::io::Result<()> {
    let mut seen = load();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut changed = false;
    for (name, tools) in now {
        let Some(entry) = entries.get(&name) else {
            continue;
        };
        let current = seen.get(&name).is_some_and(|old| {
            old.entry == *entry && old.tools == tools && at.saturating_sub(old.at) < REFRESH_SECS
        });
        if current {
            continue;
        }
        seen.insert(
            name,
            Seen {
                entry: entry.clone(),
                at,
                tools,
            },
        );
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string(&seen).map_err(std::io::Error::other)?;
    config::replace_file(&path, &text)
}

/// The record for a server, if it was made from this entry.
pub fn for_entry<'a>(
    seen: &'a BTreeMap<String, Seen>,
    name: &str,
    entry: &str,
) -> Option<&'a Seen> {
    seen.get(name).filter(|record| record.entry == entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn def(name: &str, schema: serde_json::Value) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: "Does a thing.".into(),
            input_schema: schema,
        }
    }

    #[test]
    fn a_record_is_only_used_for_the_entry_it_was_made_from() {
        let _home = crate::testing::isolated_home();
        let entries = BTreeMap::from([("notes".to_string(), "npx notes-server".to_string())]);
        record(
            BTreeMap::from([("notes".into(), vec![def("mcp__notes__search", json!({}))])]),
            &entries,
        )
        .unwrap();

        let seen = load();
        assert!(for_entry(&seen, "notes", "npx notes-server").is_some());
        assert!(for_entry(&seen, "notes", "npx other-server").is_none());
        assert!(for_entry(&seen, "elsewhere", "npx notes-server").is_none());
    }

    #[test]
    fn a_server_not_seen_this_time_keeps_its_record() {
        let _home = crate::testing::isolated_home();
        let entries = BTreeMap::from([
            ("a".to_string(), "a".to_string()),
            ("b".to_string(), "b".to_string()),
        ]);
        record(
            BTreeMap::from([
                ("a".into(), vec![def("mcp__a__x", json!({}))]),
                ("b".into(), vec![def("mcp__b__y", json!({}))]),
            ]),
            &entries,
        )
        .unwrap();
        // `b` switched off: only `a` connects.
        record(
            BTreeMap::from([("a".into(), vec![def("mcp__a__x", json!({}))])]),
            &entries,
        )
        .unwrap();
        assert!(load().contains_key("b"));
    }

    #[test]
    fn compact_prices_lower_when_there_is_prose_to_cut() {
        let seen = Seen {
            entry: String::new(),
            at: 0,
            tools: vec![def(
                "mcp__s__t",
                json!({
                    "type": "object",
                    "properties": {
                        "filter": {
                            "type": "object",
                            "description": "Narrows it. ".repeat(20),
                            "properties": { "x": { "type": "string", "description": "An x. ".repeat(40) } },
                        },
                    },
                }),
            )],
        };
        assert!(seen.tokens(true, &[]) < seen.tokens(false, &[]));
    }

    #[test]
    fn a_disabled_tool_is_not_priced() {
        let seen = Seen {
            entry: String::new(),
            at: 0,
            tools: vec![
                def("mcp__s__a", json!({"type": "object"})),
                def("mcp__s__b", json!({"type": "object"})),
            ],
        };
        let one = seen.tokens(false, &["mcp__s__b".to_string()]);
        assert!(one > 0 && one < seen.tokens(false, &[]));
    }

    #[test]
    fn an_unchanged_server_moves_its_time_on_once_it_is_stale() {
        let _home = crate::testing::isolated_home();
        let entries = BTreeMap::from([("a".to_string(), "a".to_string())]);
        let tools = || BTreeMap::from([("a".into(), vec![def("mcp__a__x", json!({}))])]);
        record(tools(), &entries).unwrap();
        let first = load()["a"].at;

        // Fresh: an ordinary reload leaves it alone.
        record(tools(), &entries).unwrap();
        assert_eq!(load()["a"].at, first);

        // Stale: it ran just now, and the record says so.
        let mut old = load();
        old.get_mut("a").unwrap().at = first - 2 * REFRESH_SECS;
        std::fs::write(path(), serde_json::to_string(&old).unwrap()).unwrap();
        record(tools(), &entries).unwrap();
        assert!(load()["a"].at >= first);
    }

    #[test]
    fn an_unreadable_file_is_no_records() {
        let _home = crate::testing::isolated_home();
        std::fs::create_dir_all(path().parent().unwrap()).unwrap();
        std::fs::write(path(), "{not json").unwrap();
        assert!(load().is_empty());
    }
}
