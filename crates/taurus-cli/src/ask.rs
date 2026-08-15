//! `ask_user`, on a terminal.
//!
//! The same bargain [`crate::permission`] makes, and for the same reason: in a
//! pipe, a git hook, or CI there is nobody to answer, and a turn that blocks
//! there has hung rather than asked. So the unattended case answers nothing at
//! once, and the tool is built to carry on from that — see
//! [`taurus_tools::view::Asker`].
//!
//! Where the app draws a card of clickable rows, this numbers the options and
//! reads a line. Enter alone skips a question, which is the same offer the card
//! makes with its "You decide" button and, in a terminal, the one people reach
//! for most.

use std::io::{IsTerminal, Write};

use async_trait::async_trait;
use taurus_tools::view::{Answer, Asker, Question, QuestionKind};

pub struct TerminalAsker {
    interactive: bool,
}

impl TerminalAsker {
    pub fn new() -> Self {
        Self {
            // Both halves, as with the permission prompt: stdin must be
            // readable for an answer to arrive and stderr must be a terminal
            // for the question to be seen.
            interactive: std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
        }
    }

    #[cfg(test)]
    fn non_interactive() -> Self {
        Self { interactive: false }
    }

    /// Puts the whole card, then reads one line per question.
    ///
    /// Everything is printed before anything is read, so the reader can see
    /// what else is coming before committing to the first answer — which is the
    /// property the card has for free and a prompt-at-a-time loop would lose.
    fn run(questions: &[Question]) -> Vec<Answer> {
        // stderr, so `taurus run > out.txt` still shows the question and the
        // redirected stdout stays clean for the model's answer.
        let mut err = std::io::stderr();
        let _ = writeln!(err, "\n  Taurus has {}:", plural(questions.len()));

        for (i, question) in questions.iter().enumerate() {
            let _ = writeln!(err, "\n  {}. {}", i + 1, question.prompt);
            for (n, option) in question.options.iter().enumerate() {
                let note = if option.note.trim().is_empty() {
                    String::new()
                } else {
                    format!("  ({})", option.note)
                };
                let _ = writeln!(err, "     [{}] {}{note}", n + 1, option.label);
            }
        }

        let mut answers = Vec::with_capacity(questions.len());
        for (i, question) in questions.iter().enumerate() {
            let hint = match question.kind {
                QuestionKind::Single => "pick one",
                QuestionKind::Multi => "pick any, comma-separated",
            };
            let hint = if question.allow_other {
                format!("{hint}, or type your own")
            } else {
                hint.to_string()
            };
            let _ = write!(err, "\n  {}. {hint} — Enter to skip: ", i + 1);
            let _ = err.flush();

            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_err() {
                // stdin closed partway through. Everything after this is
                // unanswerable, so stop asking rather than spin on EOF.
                answers.resize(questions.len(), Answer::default());
                break;
            }
            answers.push(parse(question, line.trim()));
        }
        let _ = writeln!(err);
        answers
    }
}

impl Default for TerminalAsker {
    fn default() -> Self {
        Self::new()
    }
}

fn plural(n: usize) -> String {
    if n == 1 {
        "a question".to_string()
    } else {
        format!("{n} questions")
    }
}

/// One typed line, as an answer.
///
/// Numbers select options; anything else is taken as free text where the
/// question offered it, and dropped where it did not — a typo that silently
/// became the answer to a question with fixed options would be worse than a
/// skip, because the model would act on it without either of them noticing.
fn parse(question: &Question, line: &str) -> Answer {
    if line.is_empty() {
        return Answer::default();
    }

    let mut picked = Vec::new();
    let mut unmatched = false;
    for part in line.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match part
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .and_then(|n| question.options.get(n - 1))
        {
            Some(option) => picked.push(option.label.clone()),
            None => unmatched = true,
        }
    }

    // A single-choice question keeps the first of several numbers rather than
    // rejecting the line: the intent is legible, and a re-prompt in the middle
    // of a card the user has already read is more friction than it is worth.
    if question.kind == QuestionKind::Single {
        picked.truncate(1);
    }

    let other = if unmatched && question.allow_other {
        Some(line.to_string())
    } else {
        None
    };
    Answer { picked, other }
}

#[async_trait]
impl Asker for TerminalAsker {
    async fn ask(&self, _id: &str, questions: &[Question]) -> Option<Vec<Answer>> {
        if !self.interactive {
            let mut err = std::io::stderr();
            let _ = writeln!(
                err,
                "  skipped {}: no terminal to ask on; Taurus will decide",
                plural(questions.len())
            );
            return None;
        }

        // Reading stdin blocks, which would stall the runtime the turn is
        // running on. Same move the permission prompt makes.
        let questions = questions.to_vec();
        tokio::task::spawn_blocking(move || Self::run(&questions))
            .await
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taurus_tools::view::QuestionOption;

    fn question(kind: QuestionKind, allow_other: bool) -> Question {
        Question {
            prompt: "Where should the rename land first?".into(),
            kind,
            options: vec![
                QuestionOption {
                    label: "Settings panel only".into(),
                    note: "2 files".into(),
                },
                QuestionOption {
                    label: "Every call site at once".into(),
                    note: "11 files".into(),
                },
            ],
            allow_other,
        }
    }

    #[test]
    fn a_number_picks_the_option_it_is_beside() {
        let answer = parse(&question(QuestionKind::Single, false), "2");
        assert_eq!(answer.picked, vec!["Every call site at once"]);
        assert_eq!(answer.other, None);
    }

    #[test]
    fn a_multi_question_takes_a_list() {
        let answer = parse(&question(QuestionKind::Multi, false), "1, 2");
        assert_eq!(
            answer.picked,
            vec!["Settings panel only", "Every call site at once"]
        );
    }

    #[test]
    fn a_single_question_keeps_the_first_of_several() {
        let answer = parse(&question(QuestionKind::Single, false), "2,1");
        assert_eq!(answer.picked, vec!["Every call site at once"]);
    }

    #[test]
    fn enter_alone_skips() {
        assert!(parse(&question(QuestionKind::Single, false), "").is_empty());
    }

    #[test]
    fn typed_text_counts_only_where_the_question_offered_it() {
        let offered = parse(&question(QuestionKind::Single, true), "keep an alias");
        assert_eq!(offered.other.as_deref(), Some("keep an alias"));

        // Otherwise it is a typo, and a typo must not become the answer the
        // model acts on.
        let not_offered = parse(&question(QuestionKind::Single, false), "keep an alias");
        assert!(not_offered.is_empty(), "{not_offered:?}");
    }

    #[test]
    fn an_out_of_range_number_does_not_pick_anything() {
        let answer = parse(&question(QuestionKind::Multi, false), "1,9");
        assert_eq!(answer.picked, vec!["Settings panel only"]);
    }

    #[tokio::test]
    async fn no_terminal_answers_nothing_rather_than_waiting() {
        let asked = TerminalAsker::non_interactive()
            .ask("call-1", &[question(QuestionKind::Single, false)])
            .await;
        assert!(asked.is_none());
    }
}
