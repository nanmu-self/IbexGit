use tauri::Manager;
use tracing_subscriber::{fmt, EnvFilter};

pub mod commands;
pub mod core {
    pub mod compat;
    pub mod credential;
    pub mod engine;
    pub mod error;
    pub mod graph;
    pub mod proctree;
    pub mod recovery;
    pub mod repo;
    pub mod runner;
    pub mod task;
    pub mod watcher;
    pub mod workspace;
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
            commands::git_head_message,
            commands::git_ignore_paths,
            commands::git_log,
            commands::git_graph,
            commands::git_graph_more,
            commands::git_commit_detail,
            commands::git_cherry_pick,
            commands::git_revert,
            commands::git_squash,
            commands::git_restore_file_version,
            commands::git_create_branch,
            commands::git_checkout_branch,
            commands::git_delete_branch,
            commands::git_rename_branch,
            commands::git_set_upstream,
            commands::git_tags,
            commands::git_create_tag,
            commands::git_delete_tag,
            commands::git_stash_list,
            commands::git_stash_push,
            commands::git_stash_apply,
            commands::git_stash_pop,
            commands::git_stash_drop,
            commands::git_remotes,
            commands::git_add_remote,
            commands::git_remove_remote,
            commands::git_set_remote_url,
            commands::git_prune_remote,
            commands::git_fetch,
            commands::git_pull,
            commands::git_push,
            commands::git_merge,
            commands::git_rebase,
            commands::git_reset,
            commands::git_undo_reset,
            commands::git_clean_list,
            commands::git_clean,
            commands::git_reflog,
            commands::git_branch_compare,
            commands::git_rev_list,
            commands::git_backup_list,
            commands::git_backup_delete,
            commands::git_diff,
            commands::git_stage_lines,
            commands::git_discard_lines,
            commands::git_unstage_lines,
            commands::git_file_content,
            commands::git_branches,
            commands::recovery_list,
            commands::recovery_restore,
            commands::recovery_delete,
            commands::workspace::workspace_recents,
            commands::workspace::workspace_touch_recent,
            commands::workspace::workspace_forget_recent,
            commands::workspace::workspace_load_state,
            commands::workspace::workspace_save_state,
            commands::workspace::workspace_groups,
            commands::workspace::workspace_upsert_group,
            commands::workspace::workspace_delete_group,
            commands::workspace::workspace_update_repo,
            commands::net::git_clone,
            commands::net::clone_cancel,
            commands::net::repo_init,
            commands::net::credential_list,
            commands::net::credential_delete,
            commands::net::credential_respond,
            commands::net::known_hosts_list,
            commands::net::known_hosts_remove,
            commands::net::app_set_net_config,
        ])
        .events(tauri_specta::collect_events![
            core::watcher::RepoChanged,
            commands::workspace::AppOpenPaths,
            commands::net::CloneEvent,
            core::credential::CredentialPrompt,
        ])
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
        // Single instance must be registered FIRST (tauri-plugin docs):
        // the callback runs on the primary instance when a second launch
        // arrives — focus the window and open any repo paths from argv.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            tracing::info!("second instance launched, argv={:?}", argv);
            let paths: Vec<String> = argv
                .iter()
                .skip(1)
                .filter(|a| !a.starts_with('-'))
                .filter(|a| std::path::Path::new(a).is_dir())
                .cloned()
                .collect();
            if !paths.is_empty() {
                use tauri_specta::Event as _;
                let event = commands::workspace::AppOpenPaths { paths };
                if let Err(e) = event.emit(app) {
                    tracing::warn!("failed to emit AppOpenPaths: {}", e);
                }
            }
            use tauri::Manager;
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_store::Builder::new().build())
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

            // Workspace persistence root (PLAN §4.4): {appData}/workspaces.
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to resolve app data dir");
            std::fs::create_dir_all(&data_dir).expect("failed to create app data dir");
            app.manage(crate::core::workspace::WorkspaceDir(
                data_dir.join("workspaces"),
            ));

            // Recovery snapshots root (PLAN §4.4/§4.7): {appData}/recovery.
            app.manage(crate::core::recovery::RecoveryManager::new(
                data_dir.join("recovery"),
            ));

            // CredentialBroker (P7, ADR-004): channel server + keychain + UI
            // bridge. Starts BEFORE the engine so every git process we spawn
            // already carries the helper injection.
            let bridge = NetUiBridge {
                handle: app.handle().clone(),
            };
            let broker = crate::core::credential::CredentialBroker::start(
                data_dir.clone(),
                std::sync::Arc::new(bridge),
            )?;
            app.manage(broker.clone());
            app.manage(crate::commands::net::AppEmitter(app.handle().clone()));

            // Shared spawn config: credential injection + runtime-updatable
            // proxy/SSH settings (app_set_net_config).
            let net_config = crate::core::runner::SharedNetConfig::new();
            let inject_from = |cfg: &crate::core::runner::SharedNetConfig,
                               path: &std::path::Path,
                               broker: &std::sync::Arc<
                crate::core::credential::CredentialBroker,
            >| {
                let injection = broker.spawn_injection(path);
                cfg.update(|c| {
                    c.credential = Some(crate::core::runner::CredSpawnInjection {
                        args: injection.args,
                        env: injection.env,
                    });
                });
            };
            match credential_helper_path() {
                Some(path) => inject_from(&net_config, &path, &broker),
                None if cfg!(debug_assertions) => {
                    // 开发态兜底：`tauri dev` 只构建主 bin。后台补建 helper，
                    // 完成后动态注入（后续 git 进程生效）。
                    tracing::warn!("credential-helper not built yet; building in background");
                    let net_cfg = net_config.clone();
                    let broker2 = broker.clone();
                    std::thread::spawn(move || {
                        let built = std::process::Command::new(env!("CARGO"))
                            .args(["build", "--bin", "credential-helper"])
                            .current_dir(env!("CARGO_MANIFEST_DIR"))
                            .status()
                            .map(|s| s.success())
                            .unwrap_or(false);
                        if let Some(path) = built.then(credential_helper_path).flatten() {
                            inject_from(&net_cfg, &path, &broker2);
                            tracing::info!("credential-helper built and injected");
                        } else {
                            tracing::error!("failed to build credential-helper");
                        }
                    });
                }
                None => tracing::warn!(
                    "credential-helper binary not found next to the app; \
                     credential takeover disabled"
                ),
            }
            app.manage(net_config.clone());

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
            let runner = crate::core::runner::GitProcessRunner::with_net_config(120, net_config);
            let engine: std::sync::Arc<dyn crate::core::engine::GitEngine> =
                std::sync::Arc::new(crate::core::engine::CliEngine::new(runner, "git"));
            app.manage(crate::core::repo::RepoManager::new(engine));

            // Clone task registry (P7): taskId → CancelToken.
            app.manage(crate::commands::net::CloneTasks::default());

            // State invalidation system (PLAN §4.3):
            // fs event → classify → debounce(300ms, cap 1s) → invalidate caches
            // → re-read status → emit repo://changed to the frontend.
            let hub = crate::core::watcher::WatcherHub::new();
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
            // Managed as plain WatcherHub (not Arc) — tauri State lookups are
            // type-exact, and commands declare `State<WatcherHub>`.
            app.manage(hub);

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

/// credential-helper 可执行文件路径：与主程序同目录（开发态 =
/// target/debug，安装态 = 安装目录）。
fn credential_helper_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) {
        "credential-helper.exe"
    } else {
        "credential-helper"
    };
    let path = exe.parent()?.join(name);
    path.is_file().then_some(path)
}

/// P7 UI 桥：CredentialPrompt 经 tauri-specta 事件推给前端。
struct NetUiBridge {
    handle: tauri::AppHandle,
}

impl crate::core::credential::UiBridge for NetUiBridge {
    fn show(&self, prompt: crate::core::credential::CredentialPrompt) {
        use tauri_specta::Event as _;
        if let Err(e) = prompt.emit(&self.handle) {
            tracing::warn!("failed to emit CredentialPrompt: {}", e);
        }
    }
}
