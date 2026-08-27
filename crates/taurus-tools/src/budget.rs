//! How much of the model's context window one tool answer may take.
//!
//! Every cap in this crate used to be a constant: 64 KB of command output,
//! 2000 lines of a file, 200 grep hits. One number, whatever model was asked.
//!
//! That number cannot be right twice. 64 KB is about sixteen thousand tokens,
//! which overflows an 8k local window on its own — the model is handed more
//! than it can hold and the turn fails on the request that carries it. The same
//! 64 KB is under two percent of a million-token window, where the answer is
//! cut for no reason and the model spends round trips paging back through
//! output it could have been given at once.
//!
//! So the caps are a share of the window instead, and each one is anchored so
//! that a 200,000-token window — the size the original constants were chosen
//! against — still gets the size that was written for it: 64 KB of command
//! output, 256 KB of a file, 2000 lines by default. Round numbers rather than
//! the binary ones the constants spelled them as, so 64 KB here is 64,000 bytes
//! where it used to be 65,536. Below the anchor the caps shrink and above it
//! they grow, with a floor and a ceiling on either side.
//!
//! The window is not always known: an OpenAI-compatible endpoint that never
//! declared one, a tool run outside the agent loop, a test. [`Unknown`] is that
//! case, and it answers as if the window were the anchor — so a caller that
//! cannot ask a model how big it is gets exactly the constant it got before
//! any of this existed, at every call site, by construction.
//!
//! [`Unknown`]: OutputBudget::unknown

/// Bytes per token, the same four-characters-a-token approximation the
/// compaction trigger budgets in.
///
/// Bytes rather than characters, where that crate counts characters: the two
/// part company only on non-ASCII, and an answer that is mostly non-ASCII is
/// one whose real token cost is higher than either count suggests. Erring
/// toward the smaller answer is the safe direction for a cap.
const BYTES_PER_TOKEN: usize = 4;

/// What one tool answer may take of the context window.
///
/// Cheap to copy and carried on [`crate::ToolContext`], so a tool asks the
/// budget the same way it asks for the workspace root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputBudget {
    /// The model's context window in tokens. `None` where nothing said.
    window: Option<u32>,
}

impl OutputBudget {
    /// The window these shares are anchored to.
    ///
    /// Every share its callers pass is picked so that this window reproduces
    /// the size its constant was written for. See the tests, which assert each
    /// one — a retuned share is meant to be a decision somebody made, not
    /// something that drifted.
    pub const ANCHOR_WINDOW: u32 = 200_000;

    /// The budget for a model whose window is not known.
    ///
    /// Every cap answers with what [`Self::ANCHOR_WINDOW`] would get, which is
    /// the constant it had before windows were consulted at all.
    pub const fn unknown() -> Self {
        Self { window: None }
    }

    /// The budget for a model that holds `tokens`.
    ///
    /// A window of zero is not a measurement — a provider that answered with
    /// one is a provider that did not answer — and is taken as unknown rather
    /// than as a model with no room, which would cap every tool at its floor.
    pub const fn for_window(tokens: u32) -> Self {
        if tokens == 0 {
            return Self::unknown();
        }
        Self {
            window: Some(tokens),
        }
    }

    /// The window this was built from, for callers that report on it.
    pub const fn window(self) -> Option<u32> {
        self.window
    }

    /// `share` of the window, in bytes, held between `floor` and `ceiling`.
    ///
    /// `floor` is what an answer has to be worth having — below it a cap stops
    /// bounding output and starts destroying it, and a model given three lines
    /// of a build log learns nothing and runs it again. `ceiling` is the
    /// constant this replaced: a window large enough to ask for more than
    /// anybody chose on the merits still does not get it, because past some
    /// size a single tool result is a different problem than a budget.
    pub fn bytes(self, share: f32, floor: usize, ceiling: usize) -> usize {
        let tokens = (self.window.unwrap_or(Self::ANCHOR_WINDOW) as f32 * share) as usize;
        // `min` on the floor rather than trusting the caller's pair: `clamp`
        // panics when they cross, and a panic in a tool over an arithmetic
        // detail is a worse answer than the ceiling.
        tokens
            .saturating_mul(BYTES_PER_TOKEN)
            .clamp(floor.min(ceiling), ceiling)
    }

    /// How many items of `unit_bytes` each fit in `share` of the window.
    ///
    /// For the caps counted in things rather than in bytes — lines of a file,
    /// hits from a search. `unit_bytes` is what one of them costs on average,
    /// which is an estimate about the shape of source code and is documented
    /// where it is passed.
    pub fn count(self, share: f32, unit_bytes: usize, floor: usize, ceiling: usize) -> usize {
        let room = self.bytes(share, 0, usize::MAX);
        (room / unit_bytes.max(1)).clamp(floor.min(ceiling), ceiling)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_window_answers_with_the_cap_it_had_before() {
        // The whole safety property of this module: a caller that cannot say
        // how big its model is behaves exactly as it did when these were
        // constants — not squeezed to a floor, and not handed a ceiling it was
        // never sized for either.
        let unknown = OutputBudget::unknown();
        let anchored = OutputBudget::for_window(OutputBudget::ANCHOR_WINDOW);
        assert_eq!(unknown.bytes(0.08, 4_096, 512 * 1024), 64_000);
        assert_eq!(
            unknown.bytes(0.08, 4_096, 512 * 1024),
            anchored.bytes(0.08, 4_096, 512 * 1024)
        );
        assert_eq!(unknown.count(0.10, 40, 200, 10_000), 2_000);
        assert_eq!(OutputBudget::default(), OutputBudget::unknown());
    }

    #[test]
    fn a_window_of_zero_is_not_a_window() {
        assert_eq!(OutputBudget::for_window(0), OutputBudget::unknown());
    }

    #[test]
    fn the_anchor_window_reproduces_the_old_constants() {
        // Each line here is one call site's share against the size its constant
        // was written for. If a share is ever retuned, this is what says the
        // 200k case moved.
        let at = OutputBudget::for_window(OutputBudget::ANCHOR_WINDOW);
        // `run_command` and `grep`, both 64 KB.
        assert_eq!(at.bytes(0.08, 4_096, 512 * 1024), 64_000);
        // `read_file`'s answer cap, 256 KB.
        assert_eq!(at.bytes(0.32, 8_192, 2 * 1024 * 1024), 256_000);
        // The threshold under which command output is passed through, 16 KB.
        assert_eq!(at.bytes(0.02, 2_048, 128 * 1024), 16_000);
        // `read_file`'s default of 2000 lines.
        assert_eq!(at.count(0.10, 40, 200, 10_000), 2_000);
        // `search_index`, five excerpts.
        assert_eq!(at.count(0.006, 960, 3, 24), 5);
    }

    #[test]
    fn a_small_window_gets_a_cap_it_can_actually_hold() {
        // The failure this module exists for: 64 KB is ~16k tokens, twice what
        // an 8k model holds, so the old constant could not be obeyed and
        // survive the request that carried it.
        let small = OutputBudget::for_window(8_192).bytes(0.08, 4_096, 512 * 1024);
        assert!(
            small / BYTES_PER_TOKEN < 8_192 / 2,
            "{small} bytes is still more than half an 8k window"
        );
        assert_eq!(small, 4_096, "the floor is what a tiny window lands on");
    }

    #[test]
    fn a_large_window_gets_more_than_the_old_constant() {
        let large = OutputBudget::for_window(1_000_000);
        assert_eq!(large.bytes(0.08, 4_096, 512 * 1024), 320_000);
        assert_eq!(large.count(0.10, 40, 200, 10_000), 10_000);
    }

    #[test]
    fn the_ceiling_bounds_a_window_that_would_ask_for_more() {
        // Two million tokens at a third of the window is 2.6 MB, and no single
        // tool result should be that.
        assert_eq!(
            OutputBudget::for_window(2_000_000).bytes(0.32, 8_192, 2 * 1024 * 1024),
            2 * 1024 * 1024
        );
    }

    #[test]
    fn crossed_bounds_answer_rather_than_panic() {
        // `clamp` panics when floor exceeds ceiling. A caller that gets its
        // pair backwards should get a small answer, not a crashed tool.
        assert_eq!(OutputBudget::for_window(200_000).bytes(0.5, 900, 100), 100);
        assert_eq!(
            OutputBudget::for_window(200_000).count(0.5, 40, 900, 100),
            100
        );
    }
}
