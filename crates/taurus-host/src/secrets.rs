//! Provider API keys, in the OS credential store.
//!
//! The alternative to a keychain is not "a slightly less convenient keychain",
//! it is a key in a dotfile or a key pasted into a shell profile, so this
//! exists to make the safe thing the easy one. What lives here is only the
//! secret; which provider it belongs to and how it is sent stay in
//! `providers.json`, where they are readable and diffable.
//!
//! **An environment variable wins over a stored key.** A variable someone
//! exported is an explicit act, usually in CI or a container where no keychain
//! exists at all, and a stored key silently beating it would make headless runs
//! unpredictable. The UI says when an override is in effect rather than leaving
//! the user to wonder why the key they just typed is not the one being sent.
//!
//! Not every platform has a credential store, and on Linux the Secret Service
//! may simply not be running. That is a normal state, not an error: it degrades
//! to environment variables, and [`KeyStatus`] is what tells the user which of
//! the two they are actually on.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where the key a provider will actually use came from.
///
/// Carries the variable name in the environment cases so the UI can name it.
/// A user looking at a field that says "stored" while requests come back 401
/// needs to be told which variable is beating it, not merely that one is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[ts(export)]
pub enum KeyStatus {
    /// No credential anywhere. Fine for Ollama, fatal for anything hosted.
    Missing,
    /// Read from the named environment variable; nothing is stored.
    Environment { variable: String },
    /// Read from the OS credential store.
    Keychain,
    /// Stored in the credential store, but an environment variable is being
    /// used instead. The one state worth calling out on screen.
    Overridden { variable: String },
}

impl KeyStatus {
    /// Whether a key will be sent at all.
    pub fn is_set(&self) -> bool {
        !matches!(self, Self::Missing)
    }
}

/// The credential store, where there is one.
///
/// Every platform difference is confined to this module and its twins below, so
/// the functions callers use read the same everywhere. `keyring` has no default
/// backend — a platform is either named in `Cargo.toml` or it has none — which
/// is why the fallback is a real implementation rather than a panic.
#[cfg(all(
    not(test),
    any(target_os = "macos", target_os = "windows", target_os = "linux")
))]
mod backend {
    /// Service name for every entry Taurus owns. Stable, because changing it
    /// orphans every key a user has already stored.
    const SERVICE: &str = "taurus";

    fn entry(provider_id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, provider_id)
            .map_err(|e| format!("no usable credential store on this system: {e}"))
    }

    pub fn stored(provider_id: &str) -> Option<String> {
        let key = entry(provider_id).ok()?.get_password().ok()?;
        Some(key).filter(|k| !k.trim().is_empty())
    }

    pub fn store(provider_id: &str, key: &str) -> Result<(), String> {
        entry(provider_id)?
            .set_password(key)
            .map_err(|e| format!("could not save the key: {e}"))
    }

    pub fn clear(provider_id: &str) -> Result<(), String> {
        match entry(provider_id)?.delete_credential() {
            // Removing one that was never there succeeds: the caller asked for
            // it to be gone, and it is.
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("could not remove the key: {e}")),
        }
    }

    pub fn available() -> bool {
        entry("__probe__").is_ok()
    }
}

/// Platforms with no credential store: a BSD, or any target not named in
/// `Cargo.toml`. Storing fails loudly and everything else reports nothing, so
/// the environment-variable path is all that is left — which is exactly what
/// those machines were using already.
#[cfg(all(
    not(test),
    not(any(target_os = "macos", target_os = "windows", target_os = "linux"))
))]
mod backend {
    pub fn stored(_provider_id: &str) -> Option<String> {
        None
    }

    pub fn store(_provider_id: &str, _key: &str) -> Result<(), String> {
        Err("this platform has no credential store; set an environment variable instead".into())
    }

    pub fn clear(_provider_id: &str) -> Result<(), String> {
        Ok(())
    }

    pub fn available() -> bool {
        false
    }
}

/// An in-memory store, used for the whole test suite.
///
/// Tests must never reach the real credential store: on macOS that prompts the
/// person running them, and on any platform it means a test suite writing into
/// the user's login keychain. `keyring`'s own mock is not a substitute — it
/// hands out a fresh credential per `Entry`, so nothing written survives being
/// read back.
///
/// What this leaves untested is the `keyring` call itself, which is four lines
/// of delegation. Everything with a decision in it — precedence, status,
/// blank-value handling — is the code above, and that is what these exercise.
#[cfg(test)]
mod backend {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    fn store_map() -> &'static Mutex<HashMap<String, String>> {
        static STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        STORE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn stored(provider_id: &str) -> Option<String> {
        store_map().lock().unwrap().get(provider_id).cloned()
    }

    pub fn store(provider_id: &str, key: &str) -> Result<(), String> {
        store_map()
            .lock()
            .unwrap()
            .insert(provider_id.to_string(), key.to_string());
        Ok(())
    }

    pub fn clear(provider_id: &str) -> Result<(), String> {
        store_map().lock().unwrap().remove(provider_id);
        Ok(())
    }

    pub fn available() -> bool {
        true
    }
}

/// Reads a stored key, if there is one and a store to read it from.
pub fn stored(provider_id: &str) -> Option<String> {
    backend::stored(provider_id)
}

/// Stores a key, replacing any previous one for this provider.
pub fn store(provider_id: &str, key: &str) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("the key is empty".into());
    }
    backend::store(provider_id, key)
}

/// Removes a stored key.
pub fn clear(provider_id: &str) -> Result<(), String> {
    backend::clear(provider_id)
}

/// Whether this platform can store keys at all.
///
/// The UI needs this to decide between offering a field and explaining why
/// there is none; an input that silently fails to save would be worse than no
/// input.
pub fn available() -> bool {
    backend::available()
}

/// Resolves which source a provider's key comes from.
///
/// `variable` is the provider's configured `api_key_env`, if it named one.
pub fn status(provider_id: &str, variable: Option<&str>) -> KeyStatus {
    let from_env = variable
        .filter(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
        .map(str::to_string);

    match (from_env, stored(provider_id).is_some()) {
        (Some(variable), true) => KeyStatus::Overridden { variable },
        (Some(variable), false) => KeyStatus::Environment { variable },
        (None, true) => KeyStatus::Keychain,
        (None, false) => KeyStatus::Missing,
    }
}

/// The key to send, honoring the precedence rule.
pub fn resolve(provider_id: &str, variable: Option<&str>) -> Option<String> {
    variable
        .and_then(|name| std::env::var(name).ok())
        // A variable set to nothing — `export KEY=` in a shell profile — must
        // not disable the stored key and send requests out unauthenticated.
        .filter(|key| !key.trim().is_empty())
        .or_else(|| stored(provider_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique per test: the store is process-wide and the suite runs its tests
    /// on parallel threads, so a shared id would let one test see another's key.
    fn id(test: &str) -> String {
        format!("secrets-test-{test}")
    }

    #[test]
    fn a_stored_key_comes_back() {
        let id = id("roundtrip");
        store(&id, "sk-secret").unwrap();
        assert_eq!(stored(&id).as_deref(), Some("sk-secret"));
        assert_eq!(status(&id, None), KeyStatus::Keychain);
    }

    #[test]
    fn storing_twice_replaces_rather_than_accumulates() {
        let id = id("replace");
        store(&id, "first").unwrap();
        store(&id, "second").unwrap();
        assert_eq!(stored(&id).as_deref(), Some("second"));
    }

    #[test]
    fn an_empty_key_is_refused_rather_than_stored_as_nothing() {
        let id = id("empty");
        assert!(store(&id, "   ").is_err());
        assert_eq!(status(&id, None), KeyStatus::Missing);
    }

    #[test]
    fn clearing_removes_it_and_clearing_again_still_succeeds() {
        let id = id("clear");
        store(&id, "sk-secret").unwrap();
        clear(&id).unwrap();
        assert_eq!(stored(&id), None);
        clear(&id).unwrap();
    }

    #[test]
    fn nothing_stored_and_nothing_exported_is_missing() {
        assert_eq!(status(&id("absent"), None), KeyStatus::Missing);
        assert_eq!(resolve(&id("absent"), None), None);
    }

    #[test]
    fn an_environment_variable_wins_over_a_stored_key() {
        let id = id("override");
        std::env::set_var("TAURUS_TEST_SECRET_OVERRIDE", "from-env");
        store(&id, "from-keychain").unwrap();

        assert_eq!(
            resolve(&id, Some("TAURUS_TEST_SECRET_OVERRIDE")).as_deref(),
            Some("from-env")
        );
        assert_eq!(
            status(&id, Some("TAURUS_TEST_SECRET_OVERRIDE")),
            KeyStatus::Overridden {
                variable: "TAURUS_TEST_SECRET_OVERRIDE".into()
            }
        );
        // The stored key survives the override, so unsetting the variable
        // restores it rather than leaving the provider with nothing.
        assert_eq!(stored(&id).as_deref(), Some("from-keychain"));
    }

    #[test]
    fn an_unset_variable_falls_through_to_the_stored_key() {
        let id = id("fallthrough");
        store(&id, "from-keychain").unwrap();
        assert_eq!(
            resolve(&id, Some("TAURUS_TEST_SECRET_DEFINITELY_UNSET")).as_deref(),
            Some("from-keychain")
        );
        assert_eq!(
            status(&id, Some("TAURUS_TEST_SECRET_DEFINITELY_UNSET")),
            KeyStatus::Keychain
        );
    }

    #[test]
    fn a_variable_set_to_whitespace_does_not_count_as_a_key() {
        let id = id("blank-var");
        std::env::set_var("TAURUS_TEST_SECRET_BLANK", "   ");
        store(&id, "from-keychain").unwrap();
        assert_eq!(
            resolve(&id, Some("TAURUS_TEST_SECRET_BLANK")).as_deref(),
            Some("from-keychain")
        );
        assert_eq!(
            status(&id, Some("TAURUS_TEST_SECRET_BLANK")),
            KeyStatus::Keychain
        );
    }

    #[test]
    fn an_exported_variable_with_nothing_stored_names_itself() {
        std::env::set_var("TAURUS_TEST_SECRET_ENV_ONLY", "from-env");
        assert_eq!(
            status(&id("env-only"), Some("TAURUS_TEST_SECRET_ENV_ONLY")),
            KeyStatus::Environment {
                variable: "TAURUS_TEST_SECRET_ENV_ONLY".into()
            }
        );
    }
}

/// The credential store, as the crates below this one need to see it.
///
/// One implementation over the module above, so an OAuth refresh token and a
/// provider API key are kept the same way on every platform — including the
/// platforms that have nowhere to keep either, where both fail the same way and
/// say the same thing.
///
/// This exists because the dependency runs the wrong way for those crates to
/// reach the keychain themselves: they sit *below* this one, and the platform
/// matrix is written down once, here. The trait lives at the bottom of the
/// graph in `taurus_tools::vault` so that any of them can name it —
/// deliberately not in `taurus-mcp`, whose OAuth flow is only the first caller.
pub struct Keychain;

impl taurus_tools::SecretVault for Keychain {
    fn read(&self, key: &str) -> Option<String> {
        backend::stored(key)
    }

    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        backend::store(key, value)
    }

    fn erase(&self, key: &str) -> Result<(), String> {
        backend::clear(key)
    }
}
