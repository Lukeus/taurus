//! Updating the app in place, from the signed builds a release publishes.
//!
//! The check reads `latest.json` from the newest GitHub release (the endpoint
//! is `plugins.updater` in `tauri.conf.json`). Every download is checked against
//! the public key there before anything is installed. A release built without
//! the matching private key has no signatures, and the updater refuses it
//! rather than installing something unsigned.
//!
//! Nothing here runs on its own. The window asks, shows what it found, and
//! installs only when someone presses the button.

use super::*;

use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The update the last check found, held so that installing it installs what
/// was shown rather than whatever a second check happens to return.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

/// What a check found.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct UpdateCheck {
    /// The version running now.
    pub current: String,
    #[ts(optional)]
    pub available: Option<AvailableUpdate>,
    /// Why this copy can't install an update itself, when it can't. The check
    /// still runs, so the window can still say a new version exists.
    #[ts(optional)]
    pub cannot_install: Option<String>,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct AvailableUpdate {
    pub version: String,
    /// The release notes, as written on the release.
    #[ts(optional)]
    pub notes: Option<String>,
    /// `YYYY-MM-DD`.
    #[ts(optional)]
    pub date: Option<String>,
}

/// How far a download is. `total` is 0 when the server didn't say.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct UpdateDownload {
    #[ts(type = "number")]
    pub done: u64,
    #[ts(type = "number")]
    pub total: u64,
}

/// Why this copy can't replace itself, or `None` when it can.
///
/// A dev build runs from `target/`, which isn't an installed app, so there's
/// nothing for an installer to replace. Every bundle a release builds can
/// update itself: the build knows which kind it is, and `latest.json` has an
/// entry per kind (`linux-x86_64-deb`, `windows-x86_64-msi` and so on), so a
/// `.deb` install is offered a `.deb`, not an AppImage.
fn cannot_install() -> Option<String> {
    tauri::is_dev().then(|| {
        "This is a development build. Updates install into an installed copy.".to_string()
    })
}

#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> CmdResult<UpdateCheck> {
    let current = app.package_info().version.to_string();
    let found = app
        .updater()
        .map_err(|e| format!("the updater isn't set up in this build: {e}"))?
        .check()
        .await
        .map_err(|e| {
            error!(error = %e, "update check failed");
            format!("couldn't check for updates: {e}")
        })?;

    let available = found.as_ref().map(|update| AvailableUpdate {
        version: update.version.clone(),
        notes: update.body.clone().filter(|b| !b.trim().is_empty()),
        date: update.date.map(|d| d.date().to_string()),
    });
    match &available {
        Some(a) => info!(current = %current, available = %a.version, "update available"),
        None => info!(current = %current, "no update available"),
    }
    *pending.0.lock().await = found;

    Ok(UpdateCheck {
        current,
        available,
        cannot_install: cannot_install(),
    })
}

/// Downloads the update the last check found, verifies its signature, installs
/// it, and restarts into it. Doesn't return when it succeeds.
///
/// On Windows the installer takes over and closes the app itself. Everywhere
/// else the restart is ours to do, and before it we end what the window would
/// have ended on close: the dock's shells and the agent's background commands
/// aren't in this process's tree and would outlive it.
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
    on_progress: Channel<UpdateDownload>,
) -> CmdResult<()> {
    if let Some(reason) = cannot_install() {
        return Err(reason);
    }
    let Some(update) = pending.0.lock().await.take() else {
        return Err("there's no update to install; check again first".into());
    };

    info!(version = %update.version, "downloading update");
    let mut done = 0u64;
    let result = update
        .download_and_install(
            |chunk, total| {
                done += chunk as u64;
                // A closed channel means Settings was closed. The install is
                // still worth finishing.
                let _ = on_progress.send(UpdateDownload {
                    done,
                    total: total.unwrap_or(0),
                });
            },
            || info!("update downloaded and verified; installing"),
        )
        .await;
    if let Err(e) = result {
        error!(version = %update.version, error = %e, "update failed");
        // Back in the slot, so a retry doesn't need another check.
        *pending.0.lock().await = Some(update);
        return Err(format!("the update didn't install: {e}"));
    }

    info!(version = %update.version, "update installed; restarting");
    if let Some(state) = app.try_state::<Arc<AppState>>() {
        state.terminals.close_all();
        state.voice.cancel();
        state.speaker.stop();
        state.host.stop_background();
    }
    app.restart()
}
