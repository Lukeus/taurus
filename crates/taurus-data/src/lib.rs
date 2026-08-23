//! Reading, describing, and keeping track of tabular data in a workspace.
//!
//! # The shape of it
//!
//! - [`engine`] is the contract: three read-only methods and the vocabulary
//!   they answer in. Nothing outside this crate knows what implements it.
//! - [`df`] is the implementation, today. Apache DataFusion, embedded.
//! - [`catalog`] is what a workspace has loaded — a list of pointers, kept in
//!   the harness's config home rather than in the project.
//! - [`tool`] is `load_dataset`, `profile_dataset`, and `query_data`, which
//!   the model calls.
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
//! It does not write. `query_data` runs SQL, and refuses anything that is not
//! a read — see [`engine::DataError::NotReadOnly`], which is a guarantee rather
//! than a hope, because the tool above it asks no permission. There is no
//! transform that lands anywhere, no recipe, and no derived table; their
//! absence is a scope decision rather than an oversight, and the note on
//! [`engine::Engine`] says why the line is drawn where it is.

pub mod catalog;
pub mod df;
pub mod engine;
pub mod tool;

pub use catalog::{data_dir, tables, Dataset};
pub use df::DataFusionEngine;
pub use engine::{
    ColumnHead, ColumnKind, ColumnProfile, DataError, Distinct, Engine, Format, Page, Profile,
    QueryResult, Schema, Source, ValueCount, MAX_PAGE, MAX_QUERY_ROWS,
};
pub use tool::{
    LoadDataset, ProfileDataset, QueryData, LOAD_DATASET_TOOL, PROFILE_DATASET_TOOL,
    QUERY_DATA_TOOL,
};
