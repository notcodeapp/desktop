mod agent;
mod auth;
mod bridge;
mod cli_agent;
mod cli_import;
mod external;
mod git;
mod history;
mod mcp;
mod memory;
mod ollama;
mod project;
mod providers;
mod skills;
mod state;
mod store;
mod terminal;
mod voice;

use tauri::Manager;
use tauri_plugin_deep_link::DeepLinkExt;

fn shutdown(app: &tauri::AppHandle) {
    let state = app.state::<state::AppState>();
    let bridge = state
        .bridge
        .lock()
        .ok()
        .and_then(|handle| handle.as_ref().map(|handle| handle.runtime.clone()));
    if let Some(bridge) = bridge {
        bridge.stop();
    }
    if let Ok(active) = state.generations.lock() {
        for cancel in active.values() {
            let _ = cancel.send(true);
        }
    }
    if let Ok(active) = state.model_jobs.lock() {
        for cancel in active.values() {
            cancel.send_replace(true);
        }
    }
    if let Ok(mut terminals) = state.terminals.lock() {
        terminals.clear();
    }
    if let Ok(mut mcps) = state.mcps.lock() {
        mcps.clear();
    };
    voice::stop_locked(&state);
}

/// WebView2 needs a writable profile directory. When the process starts with a
/// stripped or SYSTEM environment (installer auto-run, scheduler, restricted
/// token), %LOCALAPPDATA% resolves into system32\config\systemprofile and Edge
/// shows "Не удалось создать каталог данных" instead of the app. Point it at a
/// directory we know is writable, with a machine-wide fallback.
#[cfg(windows)]
fn prepare_webview_data_dir() {
    use std::path::PathBuf;
    let ensure_writable = |dir: &PathBuf| -> bool {
        if std::fs::create_dir_all(dir).is_err() {
            return false;
        }
        let probe = dir.join(".notcode-probe");
        match std::fs::write(&probe, b"ok") {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    };
    let candidates = [
        std::env::var("LOCALAPPDATA").ok().map(|base| PathBuf::from(base).join("dev.notcode.desktop").join("EBWebView")),
        Some(PathBuf::from("C:\\ProgramData\\NotCode\\WebView2")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if ensure_writable(&candidate) {
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &candidate);
            return;
        }
    }
    // Neither location works: leave the default behaviour rather than guess.
}

pub fn init_and_run() {
    #[cfg(windows)]
    prepare_webview_data_dir();
    let builder = tauri::Builder::default().plugin(tauri_plugin_fs::init());
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        // "Открыть с помощью" при уже запущенном приложении: файлы приходят
        // аргументами второго экземпляра.
        for argument in args.iter().skip(1) {
            if external::is_openable_argument(argument) {
                external::emit_external_open(app, argument);
            }
        }
    }));
    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = rusqlite::Connection::open(data_dir.join("workspace.sqlite3"))?;
            store::initialize(&db).map_err(std::io::Error::other)?;
            app.manage(state::AppState::new(db));
            #[cfg(desktop)]
            app.deep_link().register("notcode")?;
            auth::register_deep_link(app.handle()).map_err(std::io::Error::other)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            auth::auth_login,
            auth::auth_me,
            auth::auth_logout,
            auth::github_repos,
            bridge::bridge_addresses,
            bridge::bridge_start,
            bridge::bridge_status,
            bridge::bridge_stop,
            bridge::bridge_invite,
            bridge::bridge_pair_respond,
            bridge::bridge_revoke,
            bridge::bridge_allow_preview,
            project::project_open,
            project::project_open_path,
            project::projects_recent,
            project::project_current,
            project::project_tree,
            project::project_files_all,
            project::project_ensure_dir,
            project::file_read,
            project::file_write,
            project::project_search,
            project::project_replace,
            history::file_history_list,
            history::file_history_read,
            history::file_history_restore,
            external::external_file_read,
            external::external_file_write,
            external::external_file_pick,
            external::external_file_save_as,
            providers::provider_profiles_list,
            providers::provider_profiles_save,
            providers::provider_profiles_set_active,
            providers::provider_profiles_delete,
            providers::opencode_models,
            providers::opencode_base_url,
            providers::vault_providers_list,
            providers::vault_provider_push,
            providers::vault_provider_import,
            providers::vault_chats_list,
            providers::vault_chat_import,
            providers::google_status,
            providers::google_connect,
            providers::google_disconnect,
            providers::drive_backup_push,
            providers::drive_backup_pull,
            providers::share_chat,
            store::session_audit,
            store::sessions_list,
            store::session_create,
            store::session_messages,
            store::session_delete,
            store::session_fork,
            store::activity_stats,
            store::chats_search,
            memory::memory_get,
            memory::memory_set,
            agent::agent_tool_respond,
            agent::settings_get,
            agent::settings_set,
            agent::agent_models,
            agent::session_plan,
            agent::key_set,
            agent::key_delete,
            agent::agent_send,
            agent::agent_interrupt,
            agent::compatible_get,
            agent::compatible_models,
            agent::compatible_set,
            cli_import::cli_import_preview,
            cli_import::cli_import_chat,
            cli_agent::cli_status,
            cli_agent::cli_models,
            ollama::ollama_pull,
            ollama::ollama_import,
            ollama::ollama_cancel,
            git::git_status,
            git::git_diff,
            git::git_log,
            git::git_branches,
            git::git_commit,
            git::git_sync,
            git::git_checkout,
            git::git_clone,
            git::git_init,
            git::git_stage,
            git::git_remote,
            terminal::terminal_create,
            terminal::terminal_write,
            terminal::terminal_resize,
            terminal::terminal_close,
            mcp::mcp_list,
            mcp::mcp_add,
            mcp::mcp_remove,
            mcp::mcp_connect,
            mcp::mcp_disconnect,
            mcp::mcp_tools,
            mcp::mcp_call,
            skills::skills_list,
            skills::skill_save,
            skills::skill_delete,
            voice::voice_status,
            voice::voice_start,
            voice::voice_stop,
        ])
        .build(tauri::generate_context!())
        .expect("Unable to initialize NotCode Desktop")
        .run(|app, event| {
            match event {
                tauri::RunEvent::Ready => {
                    // Первый запуск через ассоциацию файлов: путь(-и) в argv.
                    for argument in std::env::args().skip(1) {
                        if external::is_openable_argument(&argument) {
                            external::emit_external_open(app, &argument);
                        }
                    }
                }
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => shutdown(app),
                _ => {}
            }
        });
}
