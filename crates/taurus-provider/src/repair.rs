//! Tool input JSON that is almost JSON.
//!
//! Small local models get the arguments of a call nearly right more often than
//! they get them wrong: a trailing comma, the object wrapped in a ```json fence,
//! one closing brace too many. Each of those used to become a null input and a
//! schema error, which costs a round trip and, on a model that loses the thread
//! after a few calls, often the turn.
//!
//! What this repairs is deliberately narrow. Every repair removes characters
//! the model wrote around or between complete values; none adds a value the
//! model didn't write. In particular a *truncated* object is not completed. A
//! `write_file` call cut off by the token limit can be closed into valid JSON
//! by appending `"}}`, and the result is a file with its second half missing
//! that the tool would then write without complaint. A schema error is the
//! better outcome there.

use serde_json::Value;

/// Parses `raw` as a tool's input, repairing the near-misses described in the
/// module docs. `None` when it isn't an object even after repair.
///
/// Strict parsing comes first, so input that was already valid is never
/// touched.
pub fn repair_json(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if let Ok(value @ Value::Object(_)) = serde_json::from_str(trimmed) {
        return Some(value);
    }
    let body = strip_fence(trimmed).unwrap_or(trimmed);
    let cleaned = drop_trailing_commas(body)?;
    first_value_then_closers(&cleaned).filter(Value::is_object)
}

/// The body of a single fenced block that is the whole of `s`, if it is one.
pub(crate) fn strip_fence(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("```")?;
    let rest = rest.strip_suffix("```")?;
    // The info string runs to the end of the first line.
    let newline = rest.find('\n')?;
    Some(rest[newline + 1..].trim())
}

/// Removes every comma that is followed, past whitespace, by a closing bracket.
///
/// Walks the text as JSON does, so a comma inside a string is never looked at.
/// `None` when a string is left open: that's a truncation, and see the module
/// docs for why one isn't repaired.
fn drop_trailing_commas(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escaped = false;
    for c in s.chars() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '}' | ']' => {
                let kept = out.trim_end().len();
                if out[..kept].ends_with(',') {
                    out.truncate(kept - 1);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    (!in_string).then_some(out)
}

/// The first JSON value in `s`, provided all that follows it is closing
/// brackets and whitespace: the brace too many a model writes when it loses
/// count of its nesting.
fn first_value_then_closers(s: &str) -> Option<Value> {
    let mut values = serde_json::Deserializer::from_str(s).into_iter::<Value>();
    let value = values.next()?.ok()?;
    let rest = &s[values.byte_offset()..];
    rest.chars()
        .all(|c| c.is_whitespace() || c == '}' || c == ']')
        .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_input_passes_through() {
        assert_eq!(repair_json(r#"{"path":"a"}"#), Some(json!({"path": "a"})));
    }

    #[test]
    fn a_trailing_comma_is_dropped() {
        assert_eq!(
            repair_json(r#"{"path": "a", "lines": [1, 2,],}"#),
            Some(json!({"path": "a", "lines": [1, 2]}))
        );
    }

    #[test]
    fn a_comma_inside_a_string_is_left_alone() {
        assert_eq!(
            repair_json(r#"{"text": "a,}", }"#),
            Some(json!({"text": "a,}"}))
        );
    }

    #[test]
    fn a_fence_around_the_object_is_removed() {
        assert_eq!(
            repair_json("```json\n{\"path\": \"a\"}\n```"),
            Some(json!({"path": "a"}))
        );
    }

    #[test]
    fn a_closing_brace_too_many_is_ignored() {
        assert_eq!(
            repair_json(r#"{"path": {"x": 1}}}"#),
            Some(json!({"path": {"x": 1}}))
        );
    }

    #[test]
    fn a_truncated_object_is_not_completed() {
        // The case the module docs are about: closing this would write half
        // a file.
        assert_eq!(
            repair_json(r#"{"path": "a.rs", "content": "fn main() {"#),
            None
        );
        assert_eq!(repair_json(r#"{"path": "a.rs""#), None);
    }

    #[test]
    fn something_that_is_not_an_object_is_refused() {
        assert_eq!(repair_json("[1, 2]"), None);
        assert_eq!(repair_json("read the file"), None);
        assert_eq!(repair_json(r#"{"a": 1} and then prose"#), None);
    }
}
