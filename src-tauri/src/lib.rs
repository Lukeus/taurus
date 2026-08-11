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

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = Arc::new(state::AppState::new(app.handle().clone()));
            app.manage(state.clone());

            // Skill discovery touches the filesystem; do it off the setup path
            // so the window paints immediately.
            tauri::async_runtime::spawn(async move {
                state.host.reload().await;
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
            commands::respond_permission,
            commands::list_permission_rules,
            commands::revoke_permission_rule,
            commands::list_skills,
            commands::list_proposals,
            commands::respond_skill_proposal,
            commands::set_skill_synthesis,
            commands::save_providers,
            commands::list_global_providers,
            commands::list_key_statuses,
            commands::keychain_available,
            commands::set_provider_key,
            commands::clear_provider_key,
            commands::reload_skills,
            commands::list_checkpoints,
            commands::rewind_to,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Taurus");
}
