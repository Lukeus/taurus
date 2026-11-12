//! The `taurus` command: whether it's on the PATH, and putting it there.
//!
//! The work is in `crate::cli_install`. These are only the doors to it, off
//! the runtime because both touch the filesystem and one can sit behind a
//! password prompt for as long as the user takes to answer it.

use super::*;

use tauri::AppHandle;

pub use crate::cli_install::CliStatus;

#[tauri::command]
pub async fn cli_status(app: AppHandle) -> CmdResult<CliStatus> {
    let version = app.package_info().version.to_string();
    off_runtime(move || Ok(crate::cli_install::status(&version))).await
}

/// Installs the command, and answers with what Settings should now show.
#[tauri::command]
pub async fn install_cli(app: AppHandle) -> CmdResult<CliStatus> {
    let version = app.package_info().version.to_string();
    off_runtime(move || {
        crate::cli_install::install(&version)
            .inspect_err(|e| error!(error = %e, "installing the taurus command failed"))?;
        info!(%version, "installed the taurus command");
        Ok(crate::cli_install::status(&version))
    })
    .await
}
