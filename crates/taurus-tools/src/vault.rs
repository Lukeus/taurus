//! Somewhere to keep a secret, without knowing where that somewhere is.
//!
//! # Why the trait is here and not beside the keychain
//!
//! The implementation that matters lives in `taurus_host::secrets`, over the OS
//! credential store, and every platform difference — which backend on which OS,
//! and what happens on a machine with none — is written down once there. But
//! `taurus-host` sits at the top of the crate graph. The crates that actually
//! acquire a credential all sit below it and cannot name its types.
//!
//! So the trait sits here, low enough for any of them to depend on, and the host
//! implements it downward. Today the caller is `taurus_mcp::oauth`, keeping
//! refresh tokens for MCP servers behind an OAuth consent flow. It is written as
//! a general facility rather than an OAuth one because the next thing to need a
//! credential store — a device-code sign-in, a cached service session, a token
//! for something that is not MCP at all — should implement against these three
//! methods rather than grow a second trait meaning the same thing under a
//! different name.
//!
//! # The namespace is shared
//!
//! Keys are flat strings in one namespace, shared with the provider API keys
//! `taurus_host::secrets` stores under bare provider ids. A caller keeping
//! anything else prefixes its keys so it cannot collide with those — see
//! `taurus_mcp::oauth::vault_key`, which is the worked example.

use std::collections::HashMap;
use std::sync::Mutex;

/// A credential store: three operations on opaque string values.
pub trait SecretVault: Send + Sync {
    /// The value under `key`, or `None` where there is none.
    ///
    /// `None` also covers a machine with no credential store at all, which is a
    /// normal state rather than an error — on Linux the Secret Service may
    /// simply not be running. Callers that need to tell the two apart are asking
    /// the wrong layer; that distinction belongs to whoever implements this.
    fn read(&self, key: &str) -> Option<String>;

    /// Stores `value` under `key`, replacing anything already there.
    fn write(&self, key: &str, value: &str) -> Result<(), String>;

    /// Removes `key`.
    ///
    /// Succeeds when there was nothing there. The caller asked for it to be
    /// gone, and it is.
    fn erase(&self, key: &str) -> Result<(), String>;
}

/// A vault in a `HashMap`, for tests.
///
/// Shipped rather than rewritten in each crate's test module, because a test
/// double is part of a trait's contract — that `erase` of an absent key
/// succeeds, that `read` after `write` returns the value — and three private
/// copies would be three chances to encode three different contracts.
#[derive(Default)]
pub struct InMemoryVault(Mutex<HashMap<String, String>>);

impl SecretVault for InMemoryVault {
    fn read(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }

    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }

    fn erase(&self, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn what_goes_in_comes_back_out() {
        let vault = InMemoryVault::default();
        vault.write("a", "one").unwrap();
        assert_eq!(vault.read("a").as_deref(), Some("one"));
        assert_eq!(vault.read("b"), None);
    }

    #[test]
    fn writing_twice_replaces_rather_than_appends() {
        // A refreshed token has to land on top of the one it replaces.
        let vault = InMemoryVault::default();
        vault.write("a", "one").unwrap();
        vault.write("a", "two").unwrap();
        assert_eq!(vault.read("a").as_deref(), Some("two"));
    }

    #[test]
    fn erasing_what_was_never_there_succeeds() {
        // Signing out of something never signed in to is not a failure.
        let vault = InMemoryVault::default();
        assert!(vault.erase("nothing").is_ok());
    }

    #[test]
    fn erasing_one_key_leaves_the_others() {
        let vault = InMemoryVault::default();
        vault.write("a", "one").unwrap();
        vault.write("b", "two").unwrap();
        vault.erase("a").unwrap();
        assert_eq!(vault.read("a"), None);
        assert_eq!(vault.read("b").as_deref(), Some("two"));
    }

    #[test]
    fn it_is_usable_behind_the_shared_pointer_callers_hold_it_by() {
        // Every caller holds one as `Arc<dyn SecretVault>`, so the trait has to
        // stay object-safe and the double has to satisfy `Send + Sync`.
        let vault: Arc<dyn SecretVault> = Arc::new(InMemoryVault::default());
        vault.write("a", "one").unwrap();
        assert_eq!(vault.read("a").as_deref(), Some("one"));
    }
}
