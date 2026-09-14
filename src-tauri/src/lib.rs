use tauri::Manager;
use tracing_subscriber::{fmt, EnvFilter};

pub mod commands;
pub mod core {
    pub mod compat;
    pub mod engine;
    pub mod error;
    pub mod graph;
    pub mod recovery;
    pub mod repo;
    pub mod runner;
    pub mod task;
    pub mod watcher;
}

pub fn run() {
    // Initialize tracing subscriber with env-based filter.
    let filter = match std::env::var("IBEXGIT_LOG") {
        Ok(val) => EnvFilter::new(val),
        Err(_) => {
            if cfg!(debug_assertions) {
                EnvFilter::new("debug")
            } else {
                EnvFilter::new("info")
            }
        }
    };

    fmt().with_env_filter(filter).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Detect git version on startup
            let git_caps = match crate::core::compat::GitCapabilities::detect() {
                Ok(caps) => {
                    tracing::info!("Git capabilities detected: {:?}", caps);
                    caps
                }
                Err(e) => {
                    tracing::error!("Failed to detect git capabilities: {}", e);
                    // Continue anyway; capability-gated features will disable themselves
                    Default::default()
                }
            };

            // Store in app state for later access
            app.manage(git_caps);

            // Git engine stack: runner → cli engine → repo manager.
            let runner = crate::core::runner::GitProcessRunner::new(120);
            let engine: std::sync::Arc<dyn crate::core::engine::GitEngine> =
                std::sync::Arc::new(crate::core::engine::CliEngine::new(runner, "git"));
            app.manage(crate::core::repo::RepoManager::new(engine));

            #[cfg(debug_assertions)]
            {
                let window = app.get_webview_window("main").unwrap();
                window.open_devtools();
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::git_version,
            commands::greet,
            commands::repo_open,
            commands::repo_close,
            commands::repo_list,
            commands::git_status,
            commands::git_stage,
            commands::git_unstage,
            commands::git_discard,
            commands::git_commit,
            commands::git_log,
            commands::git_diff,
            commands::git_branches,
            commands::git_checkout_branch,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
