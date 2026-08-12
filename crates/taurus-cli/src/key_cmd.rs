//! `taurus key` — API keys in the OS credential store.
//!
//! Model providers and web-search backends both keep their keys here, under
//! separate namespaces, and `--search` is what picks between them. Without it
//! a search key could only be stored from the desktop app, which would leave
//! the CLI with the worse half of a feature both frontends share.
//!
//! The secret is read from stdin, never from an argument. A key on the command
//! line is visible to every process on the machine through `ps`, and it lands
//! in the shell history of whoever typed it — which is most of what storing it
//! in a keychain was meant to avoid.

use std::io::{IsTerminal, Read};
use std::process::ExitCode;

use clap::Subcommand;
use taurus_host::{secrets::KeyStatus, Host};

#[derive(Subcommand)]
pub enum KeyCommand {
    /// Store an API key, read from stdin.
    Set {
        /// Provider id from `providers.json`, or a backend id from
        /// `search.json` with `--search`.
        id: String,
        /// Store this against a web-search backend rather than a provider.
        #[arg(long)]
        search: bool,
    },
    /// Remove a stored key.
    Clear {
        id: String,
        /// Clear a web-search backend's key rather than a provider's.
        #[arg(long)]
        search: bool,
    },
    /// Show where every key comes from.
    Status,
}

pub async fn run(host: &Host, command: KeyCommand) -> Result<ExitCode, String> {
    match command {
        KeyCommand::Set { id, search } => {
            let key = read_key()?;
            let what = if search { "search backend" } else { "provider" };
            if search {
                host.set_search_key(&id, &key).await?;
            } else {
                host.set_provider_key(&id, &key).await?;
            }

            println!("Stored the key for {what} '{id}'.");
            // Saying so at the moment of storing beats letting them discover it
            // through a 401 that names nothing.
            if let KeyStatus::Overridden { variable } = status_of(host, &id, search).await {
                println!(
                    "Note: ${variable} is set and takes precedence, so this key will not be \
                     used until that variable is unset."
                );
            }
            Ok(ExitCode::SUCCESS)
        }

        KeyCommand::Clear { id, search } => {
            if search {
                host.clear_search_key(&id).await?;
            } else {
                host.clear_provider_key(&id).await?;
            }
            println!("Removed any stored key for '{id}'.");
            Ok(ExitCode::SUCCESS)
        }

        KeyCommand::Status => {
            let providers = host.key_statuses().await;
            let backends = host.search_key_statuses();
            if providers.is_empty() && backends.is_empty() {
                println!("Nothing is configured that takes a key.");
                return Ok(ExitCode::SUCCESS);
            }

            // Labelled by section rather than listed together: the two
            // namespaces allow the same id in both, and an undifferentiated
            // list would show it twice with no way to tell which was which.
            for (heading, statuses) in [("providers", &providers), ("search", &backends)] {
                if statuses.is_empty() {
                    continue;
                }
                println!("{heading}:");
                for (id, status) in statuses.iter() {
                    println!("  {:<18} {}", id, describe(status));
                }
            }

            if !Host::keychain_available() {
                println!();
                println!(
                    "This machine has no usable credential store, so keys can only come from \
                     environment variables."
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn describe(status: &KeyStatus) -> String {
    match status {
        KeyStatus::Missing => "none".into(),
        KeyStatus::Keychain => "keychain".into(),
        KeyStatus::Environment { variable } => format!("${variable}"),
        KeyStatus::Overridden { variable } => {
            format!("${variable}  (a stored key is being overridden)")
        }
    }
}

async fn status_of(host: &Host, wanted: &str, search: bool) -> KeyStatus {
    let statuses = if search {
        host.search_key_statuses()
    } else {
        host.key_statuses().await
    };
    statuses
        .into_iter()
        .find(|(id, _)| id == wanted)
        .map(|(_, status)| status)
        .unwrap_or(KeyStatus::Missing)
}

/// Reads the key from stdin.
///
/// At a terminal the input is not echoed, so the key does not end up on screen
/// or in a scrollback buffer. Piped input is read whole instead of prompted for,
/// which is what makes `taurus key set openai < key.txt` and a pipe from a
/// password manager work without the prompt landing in the stored value.
fn read_key() -> Result<String, String> {
    let key = if std::io::stdin().is_terminal() {
        rpassword::prompt_password("Key (not echoed): ")
            .map_err(|e| format!("could not read the key: {e}"))?
    } else {
        let mut piped = String::new();
        std::io::stdin()
            .read_to_string(&mut piped)
            .map_err(|e| format!("could not read the key: {e}"))?;
        piped
    };

    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("no key was given".into());
    }
    Ok(key)
}
