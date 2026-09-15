use tauri::Manager;
use tracing_subscriber::{fmt, EnvFilter};

pub mod commands;
pub mod core {
    pub mod compat;
    pub mod engine;
    pub mod error;
    pub mod graph;
    pub mod proctree;
    pub mod recovery;
    pub mod repo;
    pub mod runner;
    pub mod task;
    pub mod watcher;
}

/// Single source of truth for the command/event surface exposed to the
/// frontend (ADR-009). `cargo test` re-exports `src/lib/git/bindings.ts`.
pub fn specta_builder<R: tauri::Runtime>() -> tauri_specta::Builder<R> {
    tauri_specta::Builder::<R>::new()
        // Commands reject with the serialized AppError (promise rejection),
        // matching the frontend toast pipeline in src/lib/git/index.ts.
        .error_handling(tauri_specta::ErrorHandlingMode::Throw)
        .commands(tauri_specta::collect_commands![
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
        .events(tauri_specta::collect_events![core::watcher::RepoChanged])
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

    let builder = specta_builder::<tauri::Wry>();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            // Regenerate TS bindings on every dev run (committed via cargo test too).
            #[cfg(debug_assertions)]
            builder
                .export(
                    specta_typescript::Typescript::default(),
                    "../src/lib/git/bindings.ts",
                )
                .expect("failed to export typescript bindings");
            builder.mount_events(app);
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

            // State invalidation system (PLAN §4.3):
            // fs event → classify → debounce(300ms, cap 1s) → invalidate caches
            // → re-read status → emit repo://changed to the frontend.
            let hub = std::sync::Arc::new(crate::core::watcher::WatcherHub::new());
            app.manage(hub.clone());
            if let Some(mut events) = hub.take_receiver() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    while let Some(ev) = events.recv().await {
                        let generation = {
                            let repos = handle.state::<crate::core::repo::RepoManager>();
                            repos.invalidate(ev.repo_id, ev.kinds).await
                        };
                        let Some(generation) = generation else {
                            continue; // event for a repo we no longer know
                        };
                        use tauri_specta::Event as _;
                        let event = crate::core::watcher::RepoChanged {
                            repo_id: ev.repo_id,
                            kinds: ev.kinds.names(),
                            generation: generation as u32,
                        };
                        if let Err(e) = event.emit(&handle) {
                            tracing::warn!("failed to emit RepoChanged: {}", e);
                        }
                    }
                });
            }

            #[cfg(debug_assertions)]
            {
                let window = app.get_webview_window("main").unwrap();
                window.open_devtools();
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
