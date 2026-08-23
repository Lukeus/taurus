//! Reading, describing, and keeping track of tabular data in a workspace.
//!
//! # The shape of it
//!
//! - [`engine`] is the contract: three read-only methods and the vocabulary
//!   they answer in. Nothing outside this crate knows what implements it.
//! - [`df`] is the implementation, today. Apache DataFusion, embedded.
//! - [`catalog`] is what a workspace has loaded — a list of pointers, kept in
//!   the harness's config home rather than in the project.
//! - [`recipe`] is a committed chain of SQL steps: what one looks like, and
//!   how one is read off disk.
//! - [`tool`] is `load_dataset`, `profile_dataset`, `query_data`, and
//!   `run_recipe`, which the model calls.
//!
//! # Why a dataset is a handle and not a view
//!
//! Everything else the harness shows the person is a payload: a table, a chart,
//! a diagram all travel whole, and a reopened conversation redraws them from
//! the call that made them. See [`taurus_tools::view`], which explains why that
//! identity is worth preserving.
//!
//! A dataset cannot work that way. It is a file with a million rows in it, and
//! the useful facts about it — how many rows, what is null, which values
//! dominate — are properties of the file *now*, not of the moment somebody
//! asked. So this crate hands out handles. A tool result carries shape and a
//! name; the rows stay where they are and are read on demand by whatever is
//! looking at them.
//!
//! That is also what keeps the transcript from filling up with data. The
//! conversation records what was decided about a dataset. The dataset itself
//! lives in the pane.
//!
//! # What writes, and what cannot
//!
//! One thing writes: [`tool::RunRecipe`], to the single path the recipe's
//! `output:` names. It asks permission and it is rewindable.
//!
//! Nothing else can. `query_data` refuses any statement that is not a read —
//! see [`engine::DataError::NotReadOnly`] — and so does every step of a
//! recipe, for a different reason: the prompt showed one path, so a step must
//! not be able to name another. Both refusals come out of the same exhaustive
//! match in [`df`], which is written out rather than defaulted so a future
//! engine release that adds a writing statement fails to compile until
//! somebody classifies it.

pub mod catalog;
pub mod df;
pub mod engine;
pub mod recipe;
pub mod tool;

pub use catalog::{data_dir, tables, Dataset};
pub use df::DataFusionEngine;
pub use engine::{
    ColumnHead, ColumnKind, ColumnProfile, DataError, Distinct, Engine, Format, Materialized, Page,
    Profile, QueryResult, Schema, Source, StepStat, ValueCount, MAX_PAGE, MAX_QUERY_ROWS,
};
pub use recipe::{Recipe, RecipeError, RECIPE_DIR};
pub use tool::{
    LoadDataset, ProfileDataset, QueryData, RunRecipe, LOAD_DATASET_TOOL, PROFILE_DATASET_TOOL,
    QUERY_DATA_TOOL, RUN_RECIPE_TOOL,
};
