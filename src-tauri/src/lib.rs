//! Taurus AI Shell — the desktop application layer.
//!
//! This crate contains no agent logic. It owns windows, IPC, and configuration,
//! and wires them to `taurus-core`, which is where the harness actually lives.

mod bridge;
mod commands;
mod state;

use std::sync::Arc;

use tauri::Manager;
use tracing_subscriber::EnvFilter;

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

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = Arc::new(state::AppState::new(app.handle().clone()));
            app.manage(state.clone());

            // Skill discovery touches the filesystem; do it off the setup path
            // so the window paints immediately. `mark_loaded` is what lets
            // `get_status` answer — until then the catalog is empty, and a
            // status read from it is a zero the rail keeps.
            tauri::async_runtime::spawn(async move {
                state.host.reload().await;
                state.mark_loaded();
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::set_workspace,
            commands::list_models,
            commands::create_session,
            commands::list_sessions,
            commands::resume_session,
            commands::send_message,
            commands::cancel_session,
            commands::close_session,
            commands::delete_session,
            commands::respond_permission,
            commands::answer_questions,
            commands::list_permission_rules,
            commands::revoke_permission_rule,
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
