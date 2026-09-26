use tauri::Manager;

fn default_log_filter() -> String {
    if cfg!(debug_assertions) {
        "debug".into()
    } else {
        "info".into()
    }
}

/// Best-effort appData resolution BEFORE the Tauri builder runs — needed to
/// attach the file log layer at init (so the swappable `EnvFilter` sits above
/// BOTH the stdout and file writers). Mirrors Tauri's `app_data_dir` rules:
/// %APPDATA% (Roaming) on Windows, ~/Library/Application Support on macOS,
/// XDG data dir on Linux.
fn app_data_dir_guess(identifier: &str) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join(identifier))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| {
            std::path::PathBuf::from(h)
                .join("Library/Application Support")
                .join(identifier)
        })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
            })?;
        Some(base.join(identifier))
    }
}

/// Two-phase tracing init (P10 高级：日志级别）: install the registry with a
/// swappable `EnvFilter` on top of the stdout formatter and (when the app
/// data dir could be resolved) a rolling file writer under
/// `{appData}/logs/ibexgit.log`. Returns the reload handle used by
/// `app_set_log_level` to adjust the filter at runtime.
fn init_tracing(filter: &str, log_dir: Option<std::path::PathBuf>) -> commands::app::FilterHandle {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let (filter_layer, filter_handle) =
        tracing_subscriber::reload::Layer::new(tracing_subscriber::EnvFilter::new(filter));

    let file_layer = log_dir.map(|dir| {
        let appender =
            tracing_appender::rolling::daily(dir.join("logs"), crate::core::logs::LOG_FILE_PREFIX);
        let (writer, guard) = tracing_appender::non_blocking(appender);
        std::mem::forget(guard); // worker runs for the app lifetime
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(writer)
    });

    match file_layer {
        Some(file) => tracing_subscriber::registry()
            .with(filter_layer)
            .with(file)
            .with(tracing_subscriber::fmt::layer())
            .init(),
        None => tracing_subscriber::registry()
            .with(filter_layer)
            .with(tracing_subscriber::fmt::layer())
            .init(),
    }
    filter_handle
}

/// 启动时一次性的日志保留清理：删除保留窗口之外的旧日期文件
/// （`core::logs::prune_old_logs`，默认保 14 天）。best-effort，
/// 失败只记日志不影响启动。
fn prune_stale_logs(identifier: &str) {
    let Some(data_dir) = app_data_dir_guess(identifier) else {
        return;
    };
    let dir = data_dir.join("logs");
    let today = chrono::Utc::now().date_naive();
    match crate::core::logs::prune_old_logs(&dir, today, crate::core::logs::LOG_KEEP_DAYS) {
        Ok(0) => {}
        Ok(n) => tracing::debug!(removed = n, "pruned expired log files"),
        Err(e) => tracing::warn!(error = %e, "log retention cleanup failed"),
    }
}

/// Read one string value from the frontend settings store
/// (`{appData}/settings.json`, a flat JSON object written by
/// tauri-plugin-store).
fn store_get_string(dir: &std::path::Path, key: &str) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("settings.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    match value.get(key) {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

pub mod commands;
pub mod core {
    pub mod ai;
    pub mod compat;
    pub mod credential;
    pub mod engine;
    pub mod error;
    pub mod folder;
    pub mod graph;
    pub mod logs;
    pub mod proctree;
    pub mod recovery;
    pub mod repo;
    pub mod runner;
    pub mod sshkeys;
    pub mod task;
    pub mod terminal;
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
            commands::git_commit_stats,
            commands::git_graph,
            commands::git_graph_more,
            commands::git_commit_detail,
            commands::git_cherry_pick,
            commands::git_revert,
            commands::git_squash,
            commands::git_restore_file_version,
            commands::git_create_branch,
            commands::git_checkout_branch,
            commands::git_remote_branches,
            commands::git_checkout_remote_branch,
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
            commands::git_mergetool,
            commands::git_conflict_list,
            commands::git_conflict_model,
            commands::git_resolve_conflict_text,
            commands::git_resolve_conflict_side,
            commands::git_operation_state,
            commands::git_operation_abort,
            commands::git_operation_continue,
            commands::git_operation_skip,
            commands::git_diff,
            commands::git_stage_lines,
            commands::git_discard_lines,
            commands::git_unstage_lines,
            commands::git_file_content,
            commands::git_file_history,
            commands::git_blame,
            commands::git_branches,
            commands::git_config_global,
            commands::git_config_local,
            commands::git_config_set_global,
            commands::git_repo_config_values,
            commands::git_repo_config_set,
            commands::git_gitignore_global,
            commands::git_commit_template,
            commands::app::app_set_log_level,
            commands::app::app_log,
            commands::app::app_check_git_path,
            commands::app::app_open_terminal,
            commands::app::app_open_folder,
            commands::ai::ai_config_get,
            commands::ai::ai_config_set,
            commands::ai::ai_set_key,
            commands::ai::ai_delete_key,
            commands::ai::ai_test_connection,
            commands::ai::ai_preview_commit_message,
            commands::ai::ai_preview_report,
            commands::ai::ai_generate_commit_message,
            commands::ai::ai_generate_report,
            commands::ai::ai_cancel,
            commands::ai::ai_export_markdown,
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
            commands::ssh::ssh_key_list,
            commands::ssh::ssh_key_generate,
            commands::ssh::ssh_key_delete,
            commands::task::task_list,
            commands::task::task_cancel,
            commands::task::task_clear_finished,
        ])
        .events(tauri_specta::collect_events![
            core::watcher::RepoChanged,
            commands::workspace::AppOpenPaths,
            commands::net::CloneEvent,
            commands::task::TaskEvent,
            core::credential::CredentialPrompt,
            commands::ai::AiEvent,
        ])
}

pub fn run() {
    let ctx = tauri::generate_context!();
    // Initialize tracing with an env-based filter.
    let filter = std::env::var("IBEXGIT_LOG").unwrap_or_else(|_| default_log_filter());
    let filter_handle = init_tracing(&filter, app_data_dir_guess(&ctx.config().identifier));
    prune_stale_logs(&ctx.config().identifier);
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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

            // P10 高级：文件日志已在 init_tracing 挂载（appData/logs）。

            // P10 设置中心：持久化的日志级别（settings.json）与自定义 git 路径。
            if std::env::var_os("IBEXGIT_LOG").is_none() {
                if let Some(level) = store_get_string(&data_dir, "logLevel") {
                    if let Ok(f) = tracing_subscriber::EnvFilter::try_new(&level) {
                        let _ = filter_handle.modify(|l| *l = f);
                    }
                }
            }
            app.manage(commands::app::LogFilter(filter_handle));
            let git_path = store_get_string(&data_dir, "gitPath")
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| "git".to_string());
            tracing::info!(%git_path, "git executable");

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
            let runner =
                crate::core::runner::GitProcessRunner::with_net_config(120, net_config.clone());
            let engine: std::sync::Arc<dyn crate::core::engine::GitEngine> =
                std::sync::Arc::new(crate::core::engine::CliEngine::new(runner, &git_path));
            app.manage(crate::core::repo::RepoManager::new(engine));

            // Clone task registry (P7): taskId → CancelToken.
            app.manage(crate::commands::net::CloneTasks::default());

            // Task center (P12): TaskManager is the truth source for user-
            // visible long tasks; every mutation is forwarded to the frontend
            // as a TaskEvent snapshot.
            let (task_tx, mut task_rx) =
                tokio::sync::mpsc::unbounded_channel::<crate::core::task::Task>();
            app.manage(crate::core::task::TaskManager::with_sink(Some(task_tx)));
            let task_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tauri_specta::Event as _;
                while let Some(task) = task_rx.recv().await {
                    let ev = commands::task::TaskEvent { task };
                    if let Err(e) = ev.emit(&task_handle) {
                        tracing::warn!("failed to emit TaskEvent: {}", e);
                    }
                }
            });

            // AI 生成任务注册表（P11）：taskId → CancelToken。
            app.manage(crate::commands::ai::AiTasks::default());

            // AI 状态（P11，ADR-013）：{appData}/ai/config.json + keyring 密钥
            // + 共享网络配置（代理继承 P7）。
            app.manage(crate::core::ai::AiState::new(
                data_dir.clone(),
                net_config.clone(),
            ));

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
        .run(ctx)
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
