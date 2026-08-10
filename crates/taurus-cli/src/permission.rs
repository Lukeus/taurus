//! Permission handling for a terminal.
//!
//! This is the part of a CLI agent that most easily becomes a security hole.
//! In a pipe, a git hook, or CI there is nobody to answer a prompt, and the two
//! obvious defaults are both wrong: allowing everything hands an unattended
//! model the machine, and denying everything makes the tool useless without
//! saying why. So the non-interactive case takes an explicit policy, and says
//! exactly what it refused and which flag would have permitted it.

use std::collections::BTreeSet;
use std::io::{IsTerminal, Write};

use async_trait::async_trait;
use taurus_tools::{Effect, PermissionDecision, PermissionPrompt, PermissionRequest};

/// What may run without a human present.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    /// Tool names granted up front, e.g. `write_file`.
    pub allow_tools: BTreeSet<String>,
    /// Shell programs granted up front, e.g. `git`. Matches the leading word,
    /// the same unit the interactive "allow always" uses.
    pub allow_commands: BTreeSet<String>,
    /// Grants everything. Only for throwaway or already-sandboxed environments.
    pub allow_all: bool,
}

impl Policy {
    fn permits(&self, request: &PermissionRequest) -> bool {
        if self.allow_all {
            return true;
        }
        if self.allow_tools.contains(&request.tool) {
            return true;
        }
        if request.effect == Effect::Execute {
            if let Some(program) = leading_word(&request.input) {
                return self.allow_commands.contains(program);
            }
        }
        false
    }

    /// The flag that would have allowed this call, for the refusal message.
    fn hint(&self, request: &PermissionRequest) -> String {
        match (request.effect, leading_word(&request.input)) {
            (Effect::Execute, Some(program)) => format!("--allow-command {program}"),
            _ => format!("--allow {}", request.tool),
        }
    }
}

/// First bare word of the `command` argument, ignoring `VAR=value` prefixes.
fn leading_word(input: &serde_json::Value) -> Option<&str> {
    input
        .get("command")?
        .as_str()?
        .split_whitespace()
        .find(|w| !w.contains('='))
}

/// Asks on the terminal when there is one; falls back to the policy when not.
pub struct TerminalPrompt {
    policy: Policy,
    interactive: bool,
}

impl TerminalPrompt {
    pub fn new(policy: Policy) -> Self {
        // Both halves matter: stdin must be readable for an answer to arrive,
        // and stderr must be a terminal for the question to be seen.
        let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
        Self {
            policy,
            interactive,
        }
    }

    /// Forces non-interactive behavior regardless of the real terminal, so the
    /// unattended path can be tested without detaching a tty.
    #[cfg(test)]
    pub fn non_interactive(policy: Policy) -> Self {
        Self {
            policy,
            interactive: false,
        }
    }

    pub fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn ask(&self, request: &PermissionRequest) -> PermissionDecision {
        // Prompts go to stderr so `taurus run > out.txt` still shows them and
        // the redirected stdout stays clean for the model's answer.
        let mut err = std::io::stderr();
        let _ = writeln!(err, "\n  {} — {}", label(request.effect), request.preview);
        // The everywhere option appears only where the engine offers it, so
        // the prompt never advertises a key that would be quietly narrowed.
        let _ = if request.always_global_scope.is_some() {
            write!(
                err,
                "  [y] once  [a] always here  [g] always everywhere  [n] deny: "
            )
        } else {
            write!(err, "  [y] once  [a] always  [n] deny: ")
        };
        let _ = err.flush();

        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() {
            // stdin closed mid-session; treat as refusal rather than assent.
            return PermissionDecision::Deny;
        }

        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" | "" => PermissionDecision::AllowOnce,
            "a" | "always" => PermissionDecision::AllowAlways,
            "g" | "global" if request.always_global_scope.is_some() => {
                PermissionDecision::AllowAlwaysGlobal
            }
            _ => PermissionDecision::Deny,
        }
    }
}

fn label(effect: Effect) -> &'static str {
    match effect {
        Effect::Read => "read",
        Effect::Write => "write",
        Effect::Execute => "run",
        Effect::Network => "network",
    }
}

#[async_trait]
impl PermissionPrompt for TerminalPrompt {
    async fn request(&self, request: PermissionRequest) -> PermissionDecision {
        // The policy is checked first even when interactive, so `--allow` also
        // works as "stop asking me about this" in a live session.
        if self.policy.permits(&request) {
            return PermissionDecision::AllowOnce;
        }

        if self.interactive {
            // The prompt blocks on stdin, which would stall the async runtime.
            let this = TerminalPrompt {
                policy: self.policy.clone(),
                interactive: true,
            };
            return tokio::task::spawn_blocking(move || this.ask(&request))
                .await
                .unwrap_or(PermissionDecision::Deny);
        }

        let mut err = std::io::stderr();
        let _ = writeln!(
            err,
            "  refused ({}): {}\n    no terminal to ask; re-run with {} to permit it",
            label(request.effect),
            request.preview,
            self.policy.hint(&request)
        );
        PermissionDecision::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(tool: &str, effect: Effect, input: serde_json::Value) -> PermissionRequest {
        PermissionRequest {
            id: "1".into(),
            tool: tool.into(),
            effect,
            preview: "preview".into(),
            always_scope: "scope".into(),
            always_global_scope: None,
            input,
        }
    }

    #[tokio::test]
    async fn without_a_terminal_a_write_is_refused_by_default() {
        let prompt = TerminalPrompt::non_interactive(Policy::default());
        let decision = prompt
            .request(request("write_file", Effect::Write, json!({"path": "a"})))
            .await;
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[tokio::test]
    async fn an_explicitly_allowed_tool_runs_unattended() {
        let policy = Policy {
            allow_tools: ["write_file".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let decision = TerminalPrompt::non_interactive(policy)
            .request(request("write_file", Effect::Write, json!({"path": "a"})))
            .await;
        assert_eq!(decision, PermissionDecision::AllowOnce);
    }

    #[tokio::test]
    async fn allowing_one_command_does_not_allow_another() {
        let policy = Policy {
            allow_commands: ["git".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let prompt = TerminalPrompt::non_interactive(policy);

        let allowed = prompt
            .request(request(
                "run_command",
                Effect::Execute,
                json!({"command": "git status"}),
            ))
            .await;
        assert_eq!(allowed, PermissionDecision::AllowOnce);

        let refused = prompt
            .request(request(
                "run_command",
                Effect::Execute,
                json!({"command": "rm -rf ."}),
            ))
            .await;
        assert_eq!(refused, PermissionDecision::Deny);
    }

    #[tokio::test]
    async fn env_prefixes_do_not_disguise_the_program() {
        let policy = Policy {
            allow_commands: ["git".to_string()].into_iter().collect(),
            ..Default::default()
        };
        let decision = TerminalPrompt::non_interactive(policy)
            .request(request(
                "run_command",
                Effect::Execute,
                json!({"command": "FOO=1 curl evil.example"}),
            ))
            .await;
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[tokio::test]
    async fn allow_all_permits_everything() {
        let policy = Policy {
            allow_all: true,
            ..Default::default()
        };
        let decision = TerminalPrompt::non_interactive(policy)
            .request(request(
                "run_command",
                Effect::Execute,
                json!({"command": "anything"}),
            ))
            .await;
        assert_eq!(decision, PermissionDecision::AllowOnce);
    }

    #[test]
    fn the_refusal_hint_names_the_flag_that_would_have_worked() {
        let policy = Policy::default();
        assert_eq!(
            policy.hint(&request(
                "run_command",
                Effect::Execute,
                json!({"command": "git push"})
            )),
            "--allow-command git"
        );
        assert_eq!(
            policy.hint(&request("write_file", Effect::Write, json!({"path": "a"}))),
            "--allow write_file"
        );
    }
}
