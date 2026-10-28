//! `${VAR}` and `${secret:NAME}` substitution for hand-written config files.
//!
//! Lives in this crate because it is the lowest one both the MCP client and the
//! web tools depend on, and two copies of a rule about where secrets come from
//! would be two chances to disagree about it.
//!
//! # Saved credentials
//!
//! `${secret:NAME}` names a credential saved in Settings › Profile and kept in
//! the OS keychain, so a token can be used without being written into a file
//! or exported into a shell. Whether a given one may be sent to a given place
//! isn't decided here: that's [`Credentials::reveal`], implemented by the layer
//! that owns the keychain and knows which hosts each credential is for. This
//! module only finds the references and says where each is going.

use crate::Scope;

/// What marks a reference as a saved credential rather than a variable.
pub const SECRET_PREFIX: &str = "secret:";

/// Where a credential is about to be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Destination<'a> {
    /// The layer that wrote the entry. A project's file is somebody else's
    /// choice of where your credential goes; your own file is yours.
    pub layer: Scope,
    /// The host an HTTP request carrying it goes to, or `None` for a program
    /// started with it in its environment.
    pub host: Option<&'a str>,
}

/// Saved credentials, and the rule for where each may be sent.
pub trait Credentials: Send + Sync {
    /// The credential called `name`, if it may go to `to`. The error is the
    /// whole sentence a person reads, naming what to change.
    fn reveal(&self, name: &str, to: &Destination<'_>) -> Result<String, String>;
}

/// What to do with a `${secret:NAME}`.
pub enum Secrets<'a> {
    /// Refuse it, saying why. For fields a credential must never go in, and
    /// for callers with no keychain to read one from.
    Refuse(&'a str),
    /// Leave it as written. For comparing two entries without reading the
    /// keychain: a reference is the same text before and after the value
    /// behind it changes, so whoever changes that value restarts what uses it.
    Keep,
    /// Fill it in, if the credential may go there.
    Reveal(&'a dyn Credentials, Destination<'a>),
}

/// Substitutes `${VAR}` from the environment, and refuses `${secret:NAME}`.
///
/// A remote endpoint almost always needs a credential, and the workspace layer
/// of a config file is meant to be hand-written and committed — a literal token
/// there is a token in the repository. Naming the variable instead is the same
/// bargain `providers.json` already makes for API keys.
///
/// A literal value still passes through untouched, so a config pasted from
/// somewhere else keeps working.
pub fn expand_env(raw: &str) -> Result<String, String> {
    expand(
        raw,
        &Secrets::Refuse("a saved credential only works in an MCP server's env or headers"),
    )
}

/// [`expand_env`], with `secrets` deciding what a `${secret:NAME}` becomes.
pub fn expand(raw: &str, secrets: &Secrets<'_>) -> Result<String, String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;

    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err("unterminated `${` — write `${VAR}`".into());
        };
        let name = &after[..end];
        if let Some(secret) = name.strip_prefix(SECRET_PREFIX) {
            check_secret_name(secret)?;
            match secrets {
                Secrets::Refuse(why) => return Err(format!("`${{{name}}}`: {why}")),
                Secrets::Keep => out.push_str(&rest[start..start + 2 + end + 1]),
                Secrets::Reveal(credentials, to) => out.push_str(&credentials.reveal(secret, to)?),
            }
        } else {
            // Unset is an error rather than an empty string: sending a blank
            // Authorization header produces a 401 that looks like a bad token
            // rather than a missing one.
            let value = std::env::var(name)
                .map_err(|_| format!("environment variable {name} is not set"))?;
            out.push_str(&value);
        }
        rest = &after[end + 1..];
    }

    out.push_str(rest);
    Ok(out)
}

/// Whether `raw` names the saved credential `name`.
pub fn references_secret(raw: &str, name: &str) -> bool {
    raw.contains(&format!("${{{SECRET_PREFIX}{name}}}"))
}

/// What a credential may be called: letters, digits, `-`, `_` and `.`, so a
/// name reads the same in a file, in a keychain entry, and on screen.
pub fn check_secret_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("`${secret:}` needs a name, like `${secret:github}`".into());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    {
        return Err(format!(
            "a credential's name is letters, digits, `-`, `_` and `.`; `{name}` has `{bad}`"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placeholder_is_read_from_the_environment() {
        // Unique per test: these mutate process-wide state, and the suite runs
        // its tests in parallel threads.
        std::env::set_var("TAURUS_TEST_ENV_EXPAND", "s3cret");
        assert_eq!(
            expand_env("Bearer ${TAURUS_TEST_ENV_EXPAND}").unwrap(),
            "Bearer s3cret"
        );
    }

    #[test]
    fn a_literal_passes_through_unchanged() {
        assert_eq!(
            expand_env("no placeholders here").unwrap(),
            "no placeholders here"
        );
    }

    #[test]
    fn an_unset_variable_names_itself() {
        let err = expand_env("${TAURUS_TEST_ENV_DEFINITELY_UNSET}").unwrap_err();
        assert!(err.contains("TAURUS_TEST_ENV_DEFINITELY_UNSET"), "{err}");
    }

    #[test]
    fn a_malformed_placeholder_is_reported_not_sent_verbatim() {
        assert!(expand_env("Bearer ${OOPS").is_err());
    }

    /// Hands out `<name>-value` to anybody, and records where it was sent.
    struct Anyone(std::sync::Mutex<Vec<(String, Option<String>)>>);

    impl Credentials for Anyone {
        fn reveal(&self, name: &str, to: &Destination<'_>) -> Result<String, String> {
            self.0
                .lock()
                .unwrap()
                .push((name.to_string(), to.host.map(str::to_string)));
            Ok(format!("{name}-value"))
        }
    }

    #[test]
    fn a_saved_credential_is_refused_where_none_may_go() {
        let err = expand_env("${secret:github}").unwrap_err();
        assert!(err.contains("${secret:github}"), "{err}");
        assert!(err.contains("env or headers"), "{err}");
    }

    #[test]
    fn a_saved_credential_is_filled_in_with_where_it_is_going() {
        let anyone = Anyone(Default::default());
        let to = Destination {
            layer: Scope::Global,
            host: Some("mcp.example.com"),
        };
        assert_eq!(
            expand("Bearer ${secret:github}", &Secrets::Reveal(&anyone, to)).unwrap(),
            "Bearer github-value"
        );
        assert_eq!(
            *anyone.0.lock().unwrap(),
            vec![("github".to_string(), Some("mcp.example.com".to_string()))]
        );
    }

    #[test]
    fn keeping_a_credential_leaves_the_reference_and_still_fills_variables() {
        // A comparison has to see a changed variable and must not read the
        // keychain to do it.
        std::env::set_var("TAURUS_TEST_ENV_KEEP", "v");
        assert_eq!(
            expand("${TAURUS_TEST_ENV_KEEP} ${secret:github}", &Secrets::Keep).unwrap(),
            "v ${secret:github}"
        );
    }

    #[test]
    fn a_credential_name_is_checked_before_anything_is_asked() {
        let anyone = Anyone(Default::default());
        let to = Destination {
            layer: Scope::Global,
            host: None,
        };
        assert!(expand("${secret:}", &Secrets::Reveal(&anyone, to)).is_err());
        assert!(expand("${secret:a b}", &Secrets::Reveal(&anyone, to)).is_err());
        assert!(anyone.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_reference_is_found_by_its_exact_name() {
        assert!(references_secret("Bearer ${secret:github}", "github"));
        assert!(!references_secret("Bearer ${secret:github-work}", "github"));
    }
}
