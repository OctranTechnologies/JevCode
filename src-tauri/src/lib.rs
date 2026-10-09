mod agent;
mod auth;
mod commands;
mod config;
mod credentials;
pub mod domain;
mod error;
mod extensions;
mod git;
mod mcp;
mod permissions;
mod persistence;
mod providers;
mod review;
mod state;
mod tools;
mod usage;
mod workspaces;

use std::{path::Path, sync::Arc, time::SystemTime};
use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let config_dir = app.path().app_config_dir()?;
            let log_dir = app.path().app_log_dir()?;
            for directory in [&data_dir, &config_dir, &log_dir] {
                std::fs::create_dir_all(directory)?;
            }
            prune_old_logs(&log_dir);
            let file_appender = tracing_appender::rolling::daily(log_dir, "jevcode.jsonl");
            let (writer, guard) = tracing_appender::non_blocking(file_appender);
            tracing_subscriber::fmt()
                .json()
                .with_ansi(false)
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "jevcode_lib=info".into()),
                )
                .with_writer(writer)
                .try_init()
                .ok();
            app.manage(guard);
            let state = Arc::new(state::AppState::new(
                persistence::Database::open(&data_dir.join("jevcode.sqlite"))?,
                config::AppConfig::load(&config_dir)?,
                credentials::CredentialStore,
            ));
            // In development, opening the application source is useful and reversible.
            #[cfg(debug_assertions)]
            if state.database.projects()?.is_empty() {
                if let Ok(directory) = std::env::current_dir() {
                    let root = if directory
                        .file_name()
                        .is_some_and(|name| name == "src-tauri")
                    {
                        directory.parent().unwrap_or(&directory)
                    } else {
                        &directory
                    };
                    if root.join("package.json").is_file() {
                        let _ = workspaces::open_project(&state.database, &root.to_string_lossy());
                    }
                }
            }
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "JevCode initialized");
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::open_project,
            commands::create_project,
            commands::remove_project_from_recents,
            commands::update_project_settings,
            commands::list_project_directory,
            commands::project_overview,
            commands::project_branches,
            commands::switch_project_branch,
            commands::reveal_project,
            commands::reveal_application_logs,
            commands::get_app_diagnostics,
            commands::check_for_updates,
            commands::open_latest_release,
            commands::run_project_terminal,
            commands::project_branch,
            commands::project_task_worktrees,
            commands::task_worktree_diff,
            commands::commit_task_worktree,
            commands::apply_task_worktree,
            commands::remove_task_worktree,
            commands::create_session,
            commands::update_session_model,
            commands::set_default_model,
            commands::toggle_model_favorite,
            commands::send_message,
            commands::rename_session,
            commands::archive_session,
            commands::resume_session,
            commands::delete_session,
            commands::duplicate_session,
            commands::fork_session,
            commands::resolve_permission,
            commands::cancel_session,
            commands::connect_provider,
            commands::disconnect_provider,
            commands::validate_provider_auth,
            commands::refresh_provider_auth,
            commands::get_provider_auth_status,
            commands::get_provider_account_info,
            commands::get_provider_available_models,
            commands::frontend_log,
            commands::set_permission_mode,
            commands::list_permission_rules,
            commands::revoke_permission_rule,
            commands::list_mcp_servers,
            commands::save_mcp_server,
            commands::set_mcp_server_enabled,
            commands::delete_mcp_server,
            commands::connect_mcp_server,
            commands::disconnect_mcp_server,
            commands::session_changes,
            commands::session_file_diff,
            commands::review_file_action,
            commands::review_all_action
        ])
        .run(tauri::generate_context!())
        .expect("JevCode could not start; check configuration and application logs");
}

fn prune_old_logs(directory: &Path) {
    const RETENTION: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 60 * 60);
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("jevcode.jsonl.") {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file() {
            continue;
        }
        let expired = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age > RETENTION);
        if expired {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
