//! Starting child processes, and ending them again.
//!
//! Two rules every child Taurus starts is held to, kept in one crate because
//! both are invisible on the machines this is written on:
//!
//! - [`no_console`]: on Windows, a child of a program with no console of its
//!   own opens a console window unless it is told not to.
//! - [`Tree`]: a child that will be ended on purpose — a hook that hits its
//!   timeout, a background command that is stopped — is ended together with
//!   everything it started, and not only itself.
//!
//! And one thing every child is given: [`path`], the PATH read again since
//! startup, so a program installed after launch is found without a restart.
//!
//! Below both `taurus-tools` and `taurus-hooks`, so the two share one copy
//! rather than keeping two in step by hand. The Windows half is compiled only
//! on Windows, and a second copy of code nobody can build locally is a second
//! place for it to be wrong.

mod console;
pub mod path;
mod tree;

pub use console::{no_console, CREATE_NO_WINDOW};
pub use tree::Tree;
