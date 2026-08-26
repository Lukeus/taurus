//! `taurus usage` — where a session's context window actually went.
//!
//! The arithmetic is not here. It is in [`taurus_host::usage`], because the
//! desktop app draws the same account and the two must not be able to disagree
//! about what a tool cost. What is here is the part that is genuinely a
//! terminal's own: column widths, what to name and what to sum up, and the
//! sentence at the end that says what to do about it.

use std::path::Path;
use std::process::ExitCode;

use taurus_host::usage::{self, UsageReport};

pub use taurus_host::usage::Fixed;

/// How many tool schemas to name individually before summarizing the rest.
///
/// A rendering decision, which is why the report carries all of them: the list
/// is cut here and nowhere earlier, so the app is free to cut it somewhere
/// else or not at all.
const SCHEMAS_LISTED: usize = 5;

pub fn run(
    workspace: &Path,
    id: Option<&str>,
    all: bool,
    fixed: &Fixed,
) -> Result<ExitCode, String> {
    let report = usage::report(workspace, id, all, fixed)?;

    if report.is_empty() {
        // Half of what this reports comes from the configuration rather than a
        // transcript, and it is the half you would want before starting rather
        // than after — so the fixed cost is still printed.
        println!(
            "No saved sessions for {}, so there is no history to account for.",
            workspace.display()
        );
        print_fixed(&report);
        return Ok(ExitCode::SUCCESS);
    }

    if all {
        println!("{} sessions in {}\n", report.sessions, workspace.display());
    }
    print_report(&report);
    print_fixed(&report);
    Ok(ExitCode::SUCCESS)
}

fn print_report(report: &UsageReport) {
    println!(
        "Turns              {}\nMessages           {}",
        report.turns, report.messages
    );
    println!(
        "Billed by provider {} in / {} out",
        thousands(report.reported_in),
        thousands(report.reported_out)
    );
    // Only when there was a cache to read from. The split is the difference
    // between a bill somebody can explain and a number that just went up:
    // a cached input token costs about a tenth of a fresh one.
    if let Some(cached) = report.cached_in.filter(|c| *c > 0) {
        let share = (cached as f64 / report.reported_in.max(1) as f64) * 100.0;
        println!(
            "  of which cached  {} ({share:.0}% of input)",
            thousands(cached)
        );
    }
    println!("Transcript holds   ~{} tokens\n", thousands(report.history));

    if report.tools.is_empty() {
        println!("No tool calls recorded.");
        return;
    }

    println!(
        "{:<22} {:>6} {:>10} {:>7}",
        "Tool", "calls", "~tokens", "share"
    );
    for tool in &report.tools {
        let failures = if tool.failures > 0 {
            format!("   {} failed", tool.failures)
        } else {
            String::new()
        };
        println!(
            "{:<22} {:>6} {:>10} {:>6}%{failures}",
            tool.name,
            tool.calls,
            thousands(tool.tokens),
            tool.share
        );
    }

    if report.repeats > 0 {
        println!(
            "\n{} of those calls repeated an earlier one exactly (~{} tokens). Same tool, \
             same input.",
            report.repeats,
            thousands(report.repeat_tokens)
        );
    }
}

/// The part of every request that is not the conversation.
///
/// History is there because the turn needed it. This is not: the system prompt
/// and every tool's schema go out again on each iteration, called or not, and
/// they are the reason a transcript worth a thousand tokens can bill twenty.
/// Printed next to the session totals because that gap is what sends people
/// looking, and the answer is almost never in the transcript.
fn print_fixed(report: &UsageReport) {
    println!(
        "\nSent again with every request  ~{} tokens",
        thousands(report.fixed_tokens())
    );
    println!(
        "  {:<26} {:>6}",
        "system prompt",
        thousands(report.system_prompt)
    );
    println!(
        "  {:<26} {:>6}",
        format!("{} tool schemas", report.schemas.len()),
        thousands(report.schema_tokens())
    );

    if report.schemas.is_empty() {
        return;
    }
    println!("\nHeaviest tool schemas");
    for schema in report.schemas.iter().take(SCHEMAS_LISTED) {
        println!("  {:<26} {:>6}", schema.name, thousands(schema.tokens));
    }
    // Said rather than left implied: a list that stops without saying so
    // reads as the whole list, and the tools it hid are exactly the ones
    // somebody deciding what to turn off would want to know about.
    if let Some(rest) = report
        .schemas
        .len()
        .checked_sub(SCHEMAS_LISTED)
        .filter(|n| *n > 0)
    {
        let hidden: u32 = report
            .schemas
            .iter()
            .skip(SCHEMAS_LISTED)
            .map(|s| s.tokens)
            .sum();
        println!("  {:<26} {:>6}", format!("{rest} more"), thousands(hidden));
    }
    println!("\nTurn off what this workspace does not use with `disabled_tools` in settings.json.");
}

/// `1234567` as `1,234,567`. Token counts are read, not computed with, and the
/// difference between 40k and 400k is the whole point of printing them.
fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_separates_every_three_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
