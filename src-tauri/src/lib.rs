//! Taurus AI Shell — the desktop application layer.
//!
//! This crate contains no agent logic. It owns windows, IPC, and configuration,
//! and wires them to `taurus-core`, which is where the harness actually lives.

mod bridge;
mod commands;
mod state;
mod terminal;

use std::sync::Arc;

use tauri::{Manager, Theme, WindowEvent};
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
    let settings = config::read_settings(Scope::Global, None);
    let stored = settings.theme;
    let resolved = match stored {
        Some(config::Theme::Light) => Some(Theme::Light),
        Some(config::Theme::Dark) => Some(Theme::Dark),
        // "Follow the system" is answered by the window, which is the only
        // thing here that has been told. A platform that will not say leaves
        // the config's value standing.
        Some(config::Theme::System) | None => window.theme().ok(),
    };

    let light = resolved == Some(Theme::Light);

    // A custom theme may have moved the ground out from under both constants.
    // Read the one file the setting names rather than the directory — this is
    // on the path to the first frame, and the rest of somebody's themes have
    // nothing to say about the colour this window opens in.
    let branded = settings
        .theme_id
        .as_deref()
        .and_then(|id| taurus_host::theme::load_theme(None, id).0)
        .and_then(|theme| {
            let palette = if light { theme.light } else { theme.dark };
            palette.get("ink").and_then(|hex| parse_color(hex))
        });

    let color = branded.unwrap_or(if light { LIGHT } else { DARK });
    let _ = window.set_background_color(Some(color));
}

/// A `#rgb`, `#rrggbb` or `#rrggbbaa` hex string as a window colour.
///
/// Alpha is read and then forced opaque: this is the ground the whole window
/// sits on, and a translucent one composites against the desktop rather than
/// against anything this app drew. Returns `None` for anything else, which
/// leaves the shipped constant standing — the same rule the rest of the theme
/// layer follows, where a value that is not a colour costs itself and nothing
/// around it.
pub(crate) fn parse_color(hex: &str) -> Option<tauri::window::Color> {
    let digits = hex.strip_prefix('#')?;
    let expand = |c: char| u8::from_str_radix(&format!("{c}{c}"), 16).ok();
    let byte = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();
    match digits.len() {
        3 | 4 => {
            let mut chars = digits.chars();
            Some(tauri::window::Color(
                expand(chars.next()?)?,
                expand(chars.next()?)?,
                expand(chars.next()?)?,
                0xff,
            ))
        }
        6 | 8 => Some(tauri::window::Color(byte(0)?, byte(2)?, byte(4)?, 0xff)),
        _ => None,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let filter = EnvFilter::try_from_env("TAURUS_LOG").unwrap_or_else(|_| {
        EnvFilter::new(
            "taurus_app=info,taurus_core=info,taurus_provider_ollama=info,taurus_skills=info",
        )
    });
    // Settings are read here rather than after the app is built, because a
    // subscriber can only be installed once and this is the only moment before
    // anything has emitted a span. Global settings only: a trace destination is
    // a property of the machine, and reading the workspace's would mean the
    // exporter changed every time somebody opened a different folder.
    let settings = taurus_host::config::load_settings(None);
    // Held for the life of the process. Dropping it flushes whatever is
    // buffered — which is why it is bound rather than discarded.
    let telemetry =
        taurus_telemetry::install(filter, "taurus-app", Some(settings.otlp_endpoint.as_str()));
    // The local half, which exists whether or not an endpoint was configured:
    // a bounded ring of finished spans in this process that nothing sends
    // anywhere. It is what the trace panel draws. Taken here because this is
    // where the subscriber was installed, and a subscriber can only be
    // installed once.
    let traces = telemetry.traces();

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
        // A shell started under a pty is not in this process's tree in any way
        // the OS will tidy up, so a closed window would otherwise leave one
        // running — along with whatever it was in the middle of. Destroyed
        // rather than CloseRequested: this is cleanup, not a veto, and it must
        // also run for a window that was closed by the platform.
        // A reload starts the frontend over with no memory of the shells it
        // opened, so anything still running belongs to nobody. The window is
        // not going away — `on_window_event` never fires — and an idle shell
        // sends no output for a failed send to notice, so this is the one
        // signal that a page has been replaced. Nothing is open on the first
        // load, which is why this can simply close everything.
        .on_page_load(|webview, payload| {
            if payload.event() != tauri::webview::PageLoadEvent::Started {
                return;
            }
            if let Some(state) = webview.app_handle().try_state::<Arc<state::AppState>>() {
                state.terminals.close_all();
            }
        })
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::Destroyed) {
                if let Some(state) = window.app_handle().try_state::<Arc<state::AppState>>() {
                    state.terminals.close_all();
                    // The same argument, for the commands the agent left
                    // running: a background `cargo watch` is no more in this
                    // process's tree than a shell under a pty is, and the
                    // window going away is the last chance to end it.
                    state.host.stop_background();
                }
            }
        })
        .setup(|app| {
            paint_window(app);

            let state = Arc::new(state::AppState::new(app.handle().clone(), traces));
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
            commands::list_datasets,
            commands::dataset_profile,
            commands::dataset_page,
            commands::open_document,
            commands::save_document,
            commands::dataset_tables,
            commands::query_data,
            commands::forget_dataset,
            commands::list_recipes,
            commands::run_recipe,
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
            commands::set_theme_id,
            commands::list_themes,
            commands::save_theme,
            commands::delete_theme,
            commands::themes_dir,
            commands::set_window_background,
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
            commands::rescan_library,
            commands::open_mcp_config,
            commands::list_mcp_servers,
            commands::mcp_catalog,
            commands::programs_on_path,
            commands::mcp_environment,
            commands::save_mcp_server,
            commands::delete_mcp_server,
            commands::set_mcp_server_disabled,
            commands::test_mcp_server,
            commands::reload_mcp,
            commands::list_checkpoints,
            commands::rewind_to,
            commands::turn_changes,
            commands::conversation_changes,
            commands::repo_status,
            commands::usage_report,
            commands::trace_report,
            commands::clear_traces,
            commands::search_sessions,
            commands::commit_turn,
            commands::terminal_open,
            commands::terminal_write,
            commands::terminal_resize,
            commands::terminal_close,
            commands::background,
            commands::background_stop,
            commands::attention,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Taurus");
}
