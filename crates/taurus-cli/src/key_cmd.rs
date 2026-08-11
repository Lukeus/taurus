//! `taurus key` — provider API keys in the OS credential store.
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
    /// Store a provider's API key, read from stdin.
    Set {
        /// Provider id, as it appears in `providers.json`.
        provider: String,
    },
    /// Remove a provider's stored key.
    Clear { provider: String },
    /// Show where each provider's key comes from.
    Status,
}

pub async fn run(host: &Host, command: KeyCommand) -> Result<ExitCode, String> {
    match command {
        KeyCommand::Set { provider } => {
            let key = read_key()?;
            host.set_provider_key(&provider, &key).await?;

            println!("Stored the key for '{provider}'.");
            // Saying so at the moment of storing beats letting them discover it
            // through a 401 that names nothing.
            if let KeyStatus::Overridden { variable } = status_of(host, &provider).await {
                println!(
                    "Note: ${variable} is set and takes precedence, so this key will not be \
                     used until that variable is unset."
                );
            }
            Ok(ExitCode::SUCCESS)
        }

        KeyCommand::Clear { provider } => {
            host.clear_provider_key(&provider).await?;
            println!("Removed any stored key for '{provider}'.");
            Ok(ExitCode::SUCCESS)
        }

        KeyCommand::Status => {
            let statuses = host.key_statuses().await;
            if statuses.is_empty() {
                println!("No providers are configured.");
                return Ok(ExitCode::SUCCESS);
            }
            for (id, status) in &statuses {
                println!("{:<20} {}", id, describe(status));
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

async fn status_of(host: &Host, provider: &str) -> KeyStatus {
    host.key_statuses()
        .await
        .into_iter()
        .find(|(id, _)| id == provider)
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
