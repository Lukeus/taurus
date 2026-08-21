//! Taurus AI Shell — the desktop application layer.
//!
//! This crate contains no agent logic. It owns windows, IPC, and configuration,
//! and wires them to `taurus-core`, which is where the harness actually lives.

mod bridge;
mod commands;
mod state;

use std::sync::Arc;

use tauri::{Manager, Theme};
use tracing_subscriber::EnvFilter;

use taurus_host::config::{self, Scope};

/// The window's own background, in the palette the app is about to paint in.
///
/// `--lk-ink` in `src/styles.css`, both halves. Kept in step by a test there,
/// which reads this file — the two cannot be derived from one another because
/// this one is needed before a stylesheet exists to read.
const DARK: tauri::window::Color = tauri::window::Color(0x0b, 0x0f, 0x14, 0xff);
const LIGHT: tauri::window::Color = tauri::window::Color(0xf7, 0xf9, 0xfb, 0xff);

// An optimized build with `cfg(dev)` still set is the one broken artifact that
// looks entirely healthy: it compiles, links, bundles, and then opens on
// "localhost refused to connect" for anyone without a Vite server on 1420. The
// flag is the `custom-protocol` feature rather than the profile, so nothing
// about `--release` implies it and nothing at runtime warns. Refusing to
// compile is the only place this can be caught before a user meets it.
#[cfg(all(not(debug_assertions), dev))]
compile_error!(
    "a release build without the `custom-protocol` feature would load `devUrl` instead of the \
     bundled frontend. Build with `pnpm tauri build`, or add `--features custom-protocol`."
);

/// Paints the window before anything is in it.
///
/// A webview opens on its host's default ground, which is white, and holds it
/// until the first paint of the document. The stylesheet already covers the
/// frame after that — see the `prefers-color-scheme` guard in `styles.css` —
/// but not this one, which is the window itself and belongs to the platform.
/// A dark-mode user saw it as a white flash on every launch.
///
/// `backgroundColor` in `tauri.conf.json` is the dark value, because that is
/// what the design system is; this corrects it for someone who has asked for
/// light. Read from the settings file rather than from the frontend's cached
/// copy, because the frontend has not run yet — that cache exists for the
/// opposite problem, the frame *after* this one.
///
/// Best-effort throughout. Every failure here costs a flash, and none of them
/// is worth refusing to open the window over.
fn paint_window(app: &tauri::App) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    // Global layer only, and deliberately: the workspace layer belongs to a
    // folder that has not been resolved yet, and a window that opened in one
    // palette because of the last project it was in would be worse than one
    // that opened in the user's own.
    let stored = config::read_settings(Scope::Global, None).theme;
    let resolved = match stored {
        Some(config::Theme::Light) => Some(Theme::Light),
        Some(config::Theme::Dark) => Some(Theme::Dark),
        // "Follow the system" is answered by the window, which is the only
        // thing here that has been told. A platform that will not say leaves
        // the config's value standing.
        Some(config::Theme::System) | None => window.theme().ok(),
    };

    if resolved == Some(Theme::Light) {
        let _ = window.set_background_color(Some(LIGHT));
    } else {
        let _ = window.set_background_color(Some(DARK));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_env("TAURUS_LOG").unwrap_or_else(|_| {
            EnvFilter::new(
                "taurus_app=info,taurus_core=info,taurus_provider_ollama=info,taurus_skills=info",
            )
        }))
        .with_target(true)
        .init();

    // Before the builder, and so before anything is spawned: an app started from
    // the Dock inherits launchd's PATH, which is four system directories and
    // nothing a user has installed. Every child this process starts afterwards
    // inherits the repaired one — MCP servers, skill interpreters, `run_command`
    // — and anything started before it would not. See `taurus_tools::login_path`.
    let path = taurus_tools::login_path::adopt();
    match &path.skipped {
        Some(reason) => tracing::info!(reason = %reason, "login shell PATH not read"),
        None if path.added.is_empty() => tracing::info!("PATH already complete"),
        None => tracing::info!(added = ?path.added, "PATH extended from the login shell"),
    }

    // The only evidence that the Windows bundle actually shipped its ConPTY
    // runtime. Without those files everything still works, except that a
    // console window appears on every pty command — a symptom visible to a user
    // on Windows in an installed build and to nothing else, which is exactly
    // the kind of packaging mistake that ships. So it is asked and answered
    // here rather than discovered. Silent off Windows, where there is nothing
    // to sideload. See `scripts/conpty.mjs`.
    match taurus_tools::sideload_status() {
        Ok(()) if cfg!(windows) => tracing::info!("ConPTY runtime found beside the executable"),
        Ok(()) => {}
        Err(missing) => tracing::warn!(
            %missing,
            "ConPTY runtime is not beside the executable; pty commands will open a console window"
        ),
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            paint_window(app);

            let state = Arc::new(state::AppState::new(app.handle().clone()));
            app.manage(state.clone());

            // Skill discovery touches the filesystem; do it off the setup path
            // so the window paints immediately. `mark_loaded` is what lets
            // `get_status` answer — until then the catalog is empty, and a
            // status read from it is a zero the rail keeps.
            //
            // The two halves of a reload are run separately here, and this is
            // the only caller that does. `mark_loaded` sits between them: what
            // the window is waiting on is a catalog with something in it, which
            // the local half produces in milliseconds, and gating it on the MCP
            // half as well made the price of becoming usable at all the sum of
            // every configured server's startup. See `Host::reload_local`.
            tauri::async_runtime::spawn(async move {
                state.host.reload_local().await;
                state.mark_loaded();
                // For a window that painted before any of this existed. The
                // frontend's own first `get_status` waits for the same load, so
                // this is the one that matters when the wait times out, and
                // the one that keeps startup a thing the shell is told about
                // rather than a thing it has to sit blocked on.
                commands::emit_status(&state).await;

                // Seconds, potentially, and nothing above needs it: an MCP tool
                // is used by a turn, and the earliest turn is the one the user
                // starts after the shell they are now looking at finishes
                // drawing. Pushed rather than waited for, so the MCP panel and
                // the tool counts fill in when the servers actually answer.
                state.host.reload_mcp().await;
                commands::emit_status(&state).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::set_workspace,
            commands::workspace_trust,
            commands::trust_workspace,
            commands::revoke_workspace_trust,
            commands::list_models,
            commands::create_session,
            commands::list_sessions,
            commands::resume_session,
            commands::read_subagent_transcript,
            commands::send_message,
            commands::cancel_session,
            commands::close_session,
            commands::delete_session,
            commands::rename_session,
            commands::switch_model,
            commands::respond_permission,
            commands::answer_questions,
            commands::list_permission_rules,
            commands::revoke_permission_rule,
            commands::list_notes,
            commands::forget_note,
            commands::list_skills,
            commands::list_instructions,
            commands::list_commands,
            commands::list_agents,
            commands::agent_roster_cost,
            commands::create_agent,
            commands::list_proposals,
            commands::respond_skill_proposal,
            commands::list_agent_proposals,
            commands::respond_agent_proposal,
            commands::set_agent_synthesis,
            commands::list_tools,
            commands::save_agent,
            commands::generate_agent,
            commands::set_skill_synthesis,
            commands::set_max_iterations,
            commands::set_agent_iterations,
            commands::set_theme,
            commands::set_embedding_model,
            commands::set_rerank,
            commands::build_index,
            commands::stop_index_build,
            commands::save_providers,
            commands::list_global_providers,
            commands::list_key_statuses,
            commands::keychain_available,
            commands::set_provider_key,
            commands::clear_provider_key,
            commands::get_search_settings,
            commands::save_search_settings,
            commands::set_search_key,
            commands::clear_search_key,
            commands::reload_config,
            commands::open_mcp_config,
            commands::list_mcp_servers,
            commands::mcp_environment,
            commands::save_mcp_server,
            commands::delete_mcp_server,
            commands::set_mcp_server_disabled,
            commands::test_mcp_server,
            commands::reload_mcp,
            commands::list_checkpoints,
            commands::rewind_to,
            commands::turn_changes,
            commands::repo_status,
            commands::commit_turn,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Taurus");
}
