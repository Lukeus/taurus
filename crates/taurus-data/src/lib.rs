//! Reading, describing, and keeping track of tabular data in a workspace.
//!
//! # The shape of it
//!
//! - [`engine`] is the contract: three read-only methods and the vocabulary
//!   they answer in. Nothing outside this crate knows what implements it.
//! - [`df`] is the implementation, today. Apache DataFusion, embedded.
//! - [`catalog`] is what a workspace has loaded — a list of pointers, kept in
//!   the harness's config home rather than in the project.
//! - [`tool`] is `load_dataset` and `profile_dataset`, which the model calls.
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
//! # What this phase does not do
//!
//! It does not write. There is no transform, no recipe, and no derived table,
//! and their absence is a scope decision rather than an oversight — see the
//! note on [`engine::Engine`].

pub mod catalog;
pub mod df;
pub mod engine;
pub mod tool;

pub use catalog::{data_dir, Dataset};
pub use df::DataFusionEngine;
pub use engine::{
    ColumnHead, ColumnKind, ColumnProfile, DataError, Distinct, Engine, Format, Page, Profile,
    Schema, Source, ValueCount, MAX_PAGE,
};
pub use tool::{LoadDataset, ProfileDataset, LOAD_DATASET_TOOL, PROFILE_DATASET_TOOL};
