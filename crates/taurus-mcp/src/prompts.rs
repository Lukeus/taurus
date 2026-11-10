//! A server's prompts: what it lists, and the text one renders to.
//!
//! The MCP *prompts* capability is a server offering canned requests by name,
//! with arguments: "review this PR", "summarise this incident". They are meant
//! to be picked by a person, which in a shell is a slash command, so that is
//! what one becomes here — `/github:review-pr 123`. The slash namespace itself
//! is `taurus-host`'s; this is the half that talks to the server.
//!
//! A prompt renders to messages, and only the text of them reaches the turn.
//! That is a narrowing: a prompt may carry an image or an embedded resource,
//! and the turn starts from what the person typed, which is text. What could
//! not be passed on is named in place rather than dropped, so the model knows
//! something was there.

use std::collections::BTreeMap;

use rmcp::model::{ContentBlock, GetPromptRequestParams, Role};
use rmcp::service::{Peer, RoleClient};
use serde::{Deserialize, Serialize};

use crate::CALL_TIMEOUT;

/// One prompt a connected server offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPrompt {
    /// The server it comes from, by its key in `mcp.json`.
    pub server: String,
    /// Its name as the server knows it, which is what `prompts/get` takes.
    pub name: String,
    /// The one line a menu shows.
    pub description: String,
    pub arguments: Vec<McpPromptArgument>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    pub description: String,
    pub required: bool,
}

impl McpPrompt {
    pub(crate) fn from_listed(server: &str, prompt: &rmcp::model::Prompt) -> Self {
        let description = prompt
            .description
            .clone()
            .or_else(|| prompt.title.clone())
            .unwrap_or_else(|| format!("The '{}' prompt from the {server} server", prompt.name));
        Self {
            server: server.to_string(),
            name: prompt.name.clone(),
            description,
            arguments: prompt
                .arguments
                .iter()
                .flatten()
                .map(|arg| McpPromptArgument {
                    name: arg.name.clone(),
                    description: arg.description.clone().unwrap_or_default(),
                    required: arg.required.unwrap_or(false),
                })
                .collect(),
        }
    }

    /// What to type after the name: `<pr> [repo]`.
    pub fn argument_hint(&self) -> Option<String> {
        if self.arguments.is_empty() {
            return None;
        }
        Some(
            self.arguments
                .iter()
                .map(|arg| {
                    if arg.required {
                        format!("<{}>", arg.name)
                    } else {
                        format!("[{}]", arg.name)
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    /// Turns what was typed after the name into the arguments the server
    /// takes.
    ///
    /// Two ways to say it, because a prompt with one argument and a prompt
    /// with four want different things from a command line:
    ///
    /// - `name=value` pairs, in any order, a quoted value counting as one.
    /// - Plain words, given to the arguments in the order the server listed
    ///   them, the last argument taking whatever is left. So a prompt whose
    ///   only argument is `topic` takes `/x:explain how caching works` whole.
    ///
    /// The two can't be mixed: `a=1 two` is refused, since which argument
    /// `two` was meant for is a guess. Every required argument has to end up
    /// with a value, and every name has to be one the prompt has.
    pub fn arguments_from(&self, line: &str) -> Result<BTreeMap<String, String>, String> {
        let words = split_words(line.trim())?;
        let mut out = BTreeMap::new();
        if words.is_empty() {
            return self.check(out);
        }

        let named: Vec<Option<(&str, &str)>> = words
            .iter()
            .map(|word| {
                word.split_once('=')
                    .filter(|(key, _)| self.arguments.iter().any(|a| a.name == *key))
            })
            .collect();

        if named.iter().all(Option::is_some) {
            for (key, value) in named.into_iter().flatten() {
                out.insert(key.to_string(), value.to_string());
            }
            return self.check(out);
        }
        if named.iter().any(Option::is_some) {
            return Err(format!(
                "Give every argument as name=value, or none of them. {}",
                self.usage()
            ));
        }

        if self.arguments.is_empty() {
            return Err(format!(
                "/{} takes no arguments.",
                command_name(&self.server, &self.name)
            ));
        }
        let last = self.arguments.len() - 1;
        for (i, arg) in self.arguments.iter().enumerate() {
            if i >= words.len() {
                break;
            }
            let value = if i == last {
                words[i..].join(" ")
            } else {
                words[i].clone()
            };
            out.insert(arg.name.clone(), value);
        }
        self.check(out)
    }

    fn check(&self, given: BTreeMap<String, String>) -> Result<BTreeMap<String, String>, String> {
        let missing: Vec<&str> = self
            .arguments
            .iter()
            .filter(|arg| arg.required && !given.contains_key(&arg.name))
            .map(|arg| arg.name.as_str())
            .collect();
        if missing.is_empty() {
            return Ok(given);
        }
        Err(format!(
            "/{} needs {}. {}",
            command_name(&self.server, &self.name),
            missing.join(", "),
            self.usage()
        ))
    }

    fn usage(&self) -> String {
        let name = command_name(&self.server, &self.name);
        let mut usage = match self.argument_hint() {
            Some(hint) => format!("Usage: /{name} {hint}"),
            None => format!("Usage: /{name}"),
        };
        for arg in &self.arguments {
            if !arg.description.is_empty() {
                usage.push_str(&format!("\n  {}: {}", arg.name, arg.description));
            }
        }
        usage
    }
}

/// The `/name` a prompt is typed as: `server:prompt`, in the lowercase-and-
/// hyphens form a slash command takes.
///
/// A server's prompt names are whatever it likes, and `review_pr` or
/// `ReviewPR` isn't something the composer reads as a command. So the name is
/// folded, and the prompt is looked up by the folded form; the server is
/// always asked for it by its own.
pub fn command_name(server: &str, prompt: &str) -> String {
    format!("{}:{}", fold(server), fold(prompt))
}

fn fold(name: &str) -> String {
    // Case is dropped rather than read as word breaks: `GitHub` is one word
    // and `ReviewPR` two, and nothing here can tell which is which.
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            'a'..='z' | '0'..='9' => out.push(c),
            'A'..='Z' => out.push(c.to_ascii_lowercase()),
            _ => {
                if !out.ends_with('-') {
                    out.push('-');
                }
            }
        }
    }
    let trimmed = out.trim_matches('-');
    // A slash command starts with a letter. See `command::split_command`.
    if trimmed.starts_with(|c: char| c.is_ascii_lowercase()) {
        trimmed.to_string()
    } else {
        format!("p-{trimmed}")
    }
}

/// Words, a double-quoted run counting as one, as a command file reads them.
fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return Err("A quote was opened and not closed.".into());
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

/// Asks a server for one prompt, and renders it as the text of a message.
pub(crate) async fn get(
    peer: &Peer<RoleClient>,
    server: &str,
    name: &str,
    arguments: BTreeMap<String, String>,
) -> Result<String, String> {
    let mut params = GetPromptRequestParams::new(name);
    if !arguments.is_empty() {
        params.arguments = Some(
            arguments
                .into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect(),
        );
    }
    let answer = tokio::time::timeout(CALL_TIMEOUT, peer.get_prompt_once(params))
        .await
        .map_err(|_| {
            format!(
                "the '{server}' server did not answer for its '{name}' prompt within {}s",
                CALL_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("the '{server}' server could not give its '{name}' prompt: {e}"))?;
    match answer {
        rmcp::model::GetPromptResponse::Complete(result) => Ok(render(&result.messages)),
        // A server asking the client a question before it answers. Nothing
        // here can ask the person on its behalf yet, the same reason
        // elicitation goes unanswered. Anything newer is treated the same:
        // not an answer this can use.
        _ => Err(format!(
            "the '{server}' server wants more input before it gives its '{name}' prompt, \
             and Taurus can't ask you for it on a server's behalf yet"
        )),
    }
}

/// The messages a prompt rendered to, as one message's text.
///
/// One user message, which is most prompts, is its own text. Anything more is
/// a little script, and its turns are labelled so the model can tell the part
/// it is meant to have said from the part it is being asked.
fn render(messages: &[rmcp::model::PromptMessage]) -> String {
    let text = |content: &ContentBlock| match content {
        ContentBlock::Text(text) => text.text.clone(),
        ContentBlock::Image(image) => {
            format!("[an image ({}) the prompt included]", image.mime_type)
        }
        ContentBlock::Audio(audio) => format!("[audio ({}) the prompt included]", audio.mime_type),
        ContentBlock::ResourceLink(link) => format!("[resource: {}]", link.uri),
        ContentBlock::Resource(_) => "[an embedded resource the prompt included]".to_string(),
        _ => "[content of a kind Taurus can't show]".to_string(),
    };
    if let [only] = messages {
        if only.role == Role::User {
            return text(&only.content);
        }
    }
    messages
        .iter()
        .map(|message| {
            let who = match message.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
            };
            format!("{who}: {}", text(&message.content))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(args: &[(&str, bool)]) -> McpPrompt {
        McpPrompt {
            server: "gh".into(),
            name: "review_pr".into(),
            description: "Review a pull request".into(),
            arguments: args
                .iter()
                .map(|(name, required)| McpPromptArgument {
                    name: name.to_string(),
                    description: String::new(),
                    required: *required,
                })
                .collect(),
        }
    }

    #[test]
    fn names_fold_to_something_the_composer_reads_as_a_command() {
        assert_eq!(command_name("github", "review_pr"), "github:review-pr");
        assert_eq!(command_name("GitHub", "Review PR"), "github:review-pr");
        assert_eq!(command_name("my server", "2fa"), "my-server:p-2fa");
    }

    #[test]
    fn a_lone_argument_takes_the_whole_line() {
        let p = prompt(&[("topic", true)]);
        let args = p.arguments_from("how caching works").unwrap();
        assert_eq!(args["topic"], "how caching works");
    }

    #[test]
    fn plain_words_fill_arguments_in_order_and_the_last_takes_the_rest() {
        let p = prompt(&[("pr", true), ("focus", false)]);
        let args = p.arguments_from("123 error handling only").unwrap();
        assert_eq!(args["pr"], "123");
        assert_eq!(args["focus"], "error handling only");
    }

    #[test]
    fn named_arguments_go_where_they_say() {
        let p = prompt(&[("pr", true), ("focus", false)]);
        let args = p.arguments_from(r#"focus="the tests" pr=7"#).unwrap();
        assert_eq!(args["pr"], "7");
        assert_eq!(args["focus"], "the tests");
    }

    #[test]
    fn a_missing_required_argument_is_named() {
        let p = prompt(&[("pr", true)]);
        let err = p.arguments_from("").unwrap_err();
        assert!(err.contains("needs pr"), "{err}");
        assert!(err.contains("/gh:review-pr <pr>"), "{err}");
    }

    #[test]
    fn mixing_named_and_plain_is_refused() {
        let p = prompt(&[("pr", true), ("focus", false)]);
        assert!(p.arguments_from("pr=1 tests").is_err());
    }

    #[test]
    fn an_equals_sign_in_a_plain_value_is_just_text() {
        let p = prompt(&[("query", true)]);
        let args = p.arguments_from("a=b").unwrap();
        assert_eq!(args["query"], "a=b");
    }

    #[test]
    fn a_single_user_message_is_its_own_text() {
        let rendered = render(&[rmcp::model::PromptMessage::new_text(Role::User, "Do it.")]);
        assert_eq!(rendered, "Do it.");
    }

    #[test]
    fn a_script_of_messages_is_labelled() {
        let rendered = render(&[
            rmcp::model::PromptMessage::new_text(Role::User, "Q"),
            rmcp::model::PromptMessage::new_text(Role::Assistant, "A"),
        ]);
        assert_eq!(rendered, "User: Q\n\nAssistant: A");
    }
}
