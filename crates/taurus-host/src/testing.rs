//! Config isolation for this crate's tests.
//!
//! `TAURUS_HOME` is process-global and the tests share a process, so isolation
//! cannot be per-test state — one test swapping the variable is visible to
//! every other test running at that moment. Config is written as a side effect
//! of ordinary work (picking a workspace persists it), so a test that reads the
//! variable at the wrong instant does not fail: it silently writes the user's
//! real `~/.taurus`. That has happened.
//!
//! Two rules make it safe. Every test that touches config holds
//! [`isolated_home`] for its whole body, so no two are ever swapping at once.
//! And the variable is never *unset* — releasing a guard points it back at a
//! process-wide scratch directory, so even an unguarded test writes somewhere
//! harmless.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use tempfile::TempDir;

use crate::config::HOME_ENV;

/// Serializes access to `TAURUS_HOME`. Poison is ignored: a panicking test
/// leaves the variable pointing at a temp directory, which is still safe, and
/// failing every later test on top of the first failure hides the real one.
fn lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where `TAURUS_HOME` points when no guard is held.
///
/// Created once and deliberately leaked: it must outlive every test in the
/// process, since it is the backstop for anything that reads config without
/// taking a guard.
fn fallback_home() -> &'static Path {
    static FALLBACK: OnceLock<PathBuf> = OnceLock::new();
    FALLBACK.get_or_init(|| {
        let dir = TempDir::new().expect("temp dir for fallback config home");
        let path = dir.path().to_path_buf();
        std::mem::forget(dir);
        path
    })
}

/// An empty `~/.taurus` for one test, held for the length of the test body.
pub struct HomeGuard {
    _lock: MutexGuard<'static, ()>,
    dir: TempDir,
}

impl HomeGuard {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // Back to the scratch directory rather than removing the variable:
        // unset means "use the real home", which is what must never happen.
        std::env::set_var(HOME_ENV, fallback_home());
    }
}

/// Points config at a fresh empty directory until the returned guard drops.
///
/// Bind it — `let _home = isolated_home();` — rather than discarding it with
/// `let _ =`, which drops it immediately and isolates nothing.
#[must_use = "config is only isolated while the guard is held"]
pub fn isolated_home() -> HomeGuard {
    let lock = lock();
    let dir = TempDir::new().expect("temp dir for isolated config");
    std::env::set_var(HOME_ENV, dir.path());
    HomeGuard { _lock: lock, dir }
}
