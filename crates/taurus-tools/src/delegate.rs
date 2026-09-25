//! What a delegate hands back when it stops.
//!
//! A delegate's last message used to be its report, which made every stop
//! look the same: "done", "I couldn't find it", and "somebody has to decide
//! this first" all arrived as a paragraph the parent had to read for intent.
//! The one that costs most is the third. A stop that says *someone* has to act
//! without saying who routes to nobody, and the work just sits.
//!
//! So a report carries a [`Disposition`], and a blocked one has to name who
//! moves next and what they have to do. The files it lists are the ones the
//! harness saw it change, never the ones it says it changed.

use std::collections::BTreeSet;
use std::sync::Mutex;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How a delegate stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "DelegateDisposition")]
pub enum Disposition {
    /// The task is complete.
    Done,
    /// It can't go on until someone does something. Always carries
    /// [`DelegateReport::owner`] and [`DelegateReport::needs`].
    Blocked,
    /// It can't be done, and the summary says why. Also what a delegate that
    /// ran out of iterations or stalled is reported as.
    Failed,
    /// Stop was pressed before it reported.
    Cancelled,
    /// It stopped without calling `finish`. The summary is its last message,
    /// which says whatever it says.
    Unreported,
}

impl Disposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unreported => "unreported",
        }
    }
}

/// Who a blocked delegate is waiting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename = "DelegateOwner")]
pub enum Owner {
    /// The agent that delegated the task.
    Parent,
    /// The person at the keyboard.
    User,
}

/// A delegate's report, as the parent's card draws it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DelegateReport {
    pub disposition: Disposition,
    /// Who moves next. Set only when blocked.
    #[ts(optional)]
    pub owner: Option<Owner>,
    /// What they have to do. Set only when blocked.
    #[ts(optional)]
    pub needs: Option<String>,
    pub summary: String,
    /// Workspace-relative paths the delegate changed, as the checkpoint path
    /// saw them.
    pub files: Vec<String>,
}

impl DelegateReport {
    /// The report as the parent model reads it.
    ///
    /// The first line is fixed in shape, `Status: <disposition>.`, because a
    /// reopened conversation rebuilds the card's chip from it: the tool result
    /// is what the transcript keeps.
    pub fn render(&self) -> String {
        let mut text = format!("Status: {}.", self.disposition.as_str());
        if let (Some(owner), Some(needs)) = (self.owner, &self.needs) {
            let who = match owner {
                Owner::Parent => "you",
                Owner::User => "the user",
            };
            text.push_str(&format!(" It needs {who} to: {}", needs.trim()));
        }
        if self.disposition == Disposition::Unreported {
            text.push_str(" It stopped without calling `finish`, so what follows is its last message, not a report.");
        }
        let summary = self.summary.trim();
        text.push_str("\n\n");
        text.push_str(if summary.is_empty() {
            "(nothing)"
        } else {
            summary
        });
        if !self.files.is_empty() {
            text.push_str(&format!("\n\nFiles changed: {}", self.files.join(", ")));
        }
        text
    }
}

/// The files one delegate changed, collected as its calls make them.
///
/// A delegate's writes are recorded into the turn that spawned it, which
/// keeps one pre-image per file per turn. That's right for undo and useless for
/// attribution: a file the parent already touched this turn is not recorded
/// again, and two delegates share one set. So each delegate gets one of
/// these, and the registry adds to it beside the recorder.
#[derive(Debug, Default)]
pub struct Touched(Mutex<BTreeSet<String>>);

impl Touched {
    pub fn add<'a>(&self, paths: impl IntoIterator<Item = &'a str>) {
        let mut set = self.0.lock().unwrap_or_else(|e| e.into_inner());
        set.extend(paths.into_iter().map(str::to_string));
    }

    /// Sorted, since it's a `BTreeSet`.
    pub fn paths(&self) -> Vec<String> {
        let set = self.0.lock().unwrap_or_else(|e| e.into_inner());
        set.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(disposition: Disposition) -> DelegateReport {
        DelegateReport {
            disposition,
            owner: None,
            needs: None,
            summary: "Found it in lib.rs.".into(),
            files: vec![],
        }
    }

    #[test]
    fn the_first_line_is_the_status() {
        for d in [
            Disposition::Done,
            Disposition::Blocked,
            Disposition::Failed,
            Disposition::Cancelled,
            Disposition::Unreported,
        ] {
            let text = report(d).render();
            assert!(
                text.starts_with(&format!("Status: {}.", d.as_str())),
                "{text}"
            );
        }
    }

    #[test]
    fn a_blocked_report_names_who_moves_next() {
        let text = DelegateReport {
            owner: Some(Owner::User),
            needs: Some("say which config file is the real one".into()),
            ..report(Disposition::Blocked)
        }
        .render();
        assert!(text.starts_with(
            "Status: blocked. It needs the user to: say which config file is the real one"
        ));

        let text = DelegateReport {
            owner: Some(Owner::Parent),
            needs: Some("give me the failing test's name".into()),
            ..report(Disposition::Blocked)
        }
        .render();
        assert!(text.contains("It needs you to: give me the failing test's name"));
    }

    #[test]
    fn files_are_listed_after_the_summary() {
        let text = DelegateReport {
            files: vec!["a.rs".into(), "b/c.rs".into()],
            ..report(Disposition::Done)
        }
        .render();
        assert!(text.ends_with("Found it in lib.rs.\n\nFiles changed: a.rs, b/c.rs"));
    }

    #[test]
    fn touched_paths_are_sorted_and_deduplicated() {
        let touched = Touched::default();
        touched.add(["b.rs", "a.rs"]);
        touched.add(["a.rs"]);
        assert_eq!(touched.paths(), ["a.rs", "b.rs"]);
    }
}
