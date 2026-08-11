//! `${VAR}` substitution for hand-written config files.
//!
//! Lives in this crate because it is the lowest one both the MCP client and the
//! web tools depend on, and two copies of a rule about where secrets come from
//! would be two chances to disagree about it.

/// Substitutes `${VAR}` from the environment.
///
/// A remote endpoint almost always needs a credential, and the workspace layer
/// of a config file is meant to be hand-written and committed — a literal token
/// there is a token in the repository. Naming the variable instead is the same
/// bargain `providers.json` already makes for API keys.
///
/// A literal value still passes through untouched, so a config pasted from
/// somewhere else keeps working.
pub fn expand_env(raw: &str) -> Result<String, String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;

    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err("unterminated `${` — write `${VAR}`".into());
        };
        let name = &after[..end];
        // Unset is an error rather than an empty string: sending a blank
        // Authorization header produces a 401 that looks like a bad token
        // rather than a missing one.
        let value =
            std::env::var(name).map_err(|_| format!("environment variable {name} is not set"))?;
        out.push_str(&value);
        rest = &after[end + 1..];
    }

    out.push_str(rest);
    Ok(out)
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
}
