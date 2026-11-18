# AGENTS.md — AI 编码代理指南

IbexGit：本地优先的 Git 图形客户端（Tauri 2 + SvelteKit 5 + TypeScript + Rust）。
阶段路线图见 README 的进度表。

## 技术栈

- **前端**：SvelteKit 5 SPA（Svelte 5 runes）+ Tailwind 4 + shadcn-svelte；Shiki 高亮；CodeMirror 6（冲突编辑，动态加载）
- **后端**：Tauri 2，git 走系统 git CLI（≥ 2.40，非 libgit2）；tauri-specta 生成 TS 绑定；tracing 日志（stdout + 按天滚动文件）
- **凭据**：独立 helper 子进程（credential 协议 + askpass）+ OS keychain（keyring）

## 常用命令

```bash
pnpm install              # 用 pnpm，CI 用 frozen-lockfile
pnpm check                # svelte-check，必须 0 error
pnpm build                # 前端构建
pnpm test:unit           # path.ts 纯函数断言（node 直跑，需 Node ≥ 22.6）
pnpm tauri dev            # 桌面开发模式

cd src-tauri
cargo fmt --all                             # 提交前必跑，CI 用 --check
cargo clippy --all-targets -- -D warnings   # 0 warning
cargo test                # 全部测试；同时重新生成 bindings.ts
```

提交前全套：`cargo fmt` → `clippy -D warnings` → `cargo test` → `pnpm check` → `pnpm build`。
测试需 `git`（≥ 2.40）在 PATH。

## 目录速查

```
src/lib/git/          # 命令 API（repo/git/recovery/net/app/ai/workspace）+ normalizeError
  bindings.ts         # tauri-specta 生成，勿手改
src/lib/diff/         # diff 行模型（虚拟滚动）、词级差异、高亮
src/lib/components/   # ui/layout/workspace/diff/history/refs/merge/file/credential/ai/settings/palette/…
src/lib/keyboard/     # keymap.ts 键位单一数据源
src/lib/stores/       # runes 状态（*.svelte.ts）；toast.ts 唯一 writable；refbus.ts 回调总线
src/lib/i18n/         # 自研 runes i18n，字典 dictionaries/{zh-CN,en}.json
src/lib/features.ts   # 功能注册表：菜单/命令面板/快捷键同源
src-tauri/src/core/
  engine/             # GitEngine trait + cli.rs 实现 + parse.rs 解析器（唯一 git 入口）
  runner.rs           # GitProcessRunner：spawn/超时/kill/stdin 模式
  repo.rs             # RepoManager：写 gate / 读信号量 / 缓存
  watcher.rs          # notify → 防抖 → repo://changed 失效事件
  credential.rs ai/ sshkeys.rs task.rs graph.rs recovery.rs …
src-tauri/src/commands/   # #[tauri::command] 薄封装：mod + ai/app/net/ssh/workspace
src-tauri/src/bin/credential-helper.rs   # 独立凭据 helper
src-tauri/tests/      # 集成测试（含 bindings.rs 同步校验）
```

## 规范与红线

1. **GitEngine trait 是 git 操作唯一入口**（ADR-001）。不要在 runner.rs 之外
   `Command::new("git")`；新 git 能力 = trait 方法 + cli.rs 实现 + parse.rs 纯函数解析器 + 单测。
2. **git 子进程永不交互**：stdin 只允许 `Null`（默认）或 `Feed`（一次写入即关）；
   强制 `GIT_TERMINAL_PROMPT=0`、`GIT_EDITOR=true`、`-c core.quotepath=false`。
   凭据走 helper-first（ADR-014，追加链尾注入，独立进程回连 CredentialBroker）。
   **只读命令一律带 `--no-optional-locks`**：否则 `status` / `diff` 会把刷新后的
   stat 缓存写回 `.git/index`，而我们正 watch 这个文件——读操作自己触发一次
   INDEX 事件，把即将填好的缓存作废，等于每次操作多一次全量重读（实测单次外部
   `git add` 从 3 次失效降到 1 次）。读不得写。
3. **缓存只作展示加速，`.git` 与工作区是唯一真相源**。所有变更（含自身写操作）
   走 watcher 失效 → `repo://changed`，不为内部写操作开快速通道。
   唯一例外是**写后失效**：写命令的 `WriteGate` guard 析构时清空本仓展示缓存
   （只清、不预读），让紧随其后的读取必然穿透到 git——否则写命令返回后那次
   刷新会命中写前快照，要等 watcher 防抖（300ms–1s）+ 全量重读才纠正。
   它不写入任何数据，缓存只会比 git 旧、不会更新，真相源方向不变；watcher
   链路照旧兜底。
   派生规则见 `RepoManager::invalidate`：失效时的**防抖重读是投机性的**，
   仅当「最后一次 status 读取的起点晚于该批事件的最新观测时刻」时才跳过——
   那次读取已经覆盖了这批变更，重读只会得到同一答案（`Session.last_read_started`
   vs `RepoEvent.observed_at`）。反过来，读取起点早于变更时结果不入缓存
   （`Session.last_change_at`），避免把读到 index 中间态的快照固化成"新鲜"。
   与此配套，watcher 的事件处理**先等 `WriteGate::wait_idle()`**（按事件 spawn，
   不阻塞其它仓库）：自家写引起的 fs 事件通常在命令还在跑时就到了，此时重读会
   和 git 抢 index，而且结果马上被 guard 析构作废（实测 276ms 的 commit 被拖到
   637ms）。
   推论：**git 子进程数才是第一成本**，而单个子进程的成本几乎全在"创建"上
   （实测 78ms 里 40ms 是 CreateProcess + 线程恢复，git 本身只占 ~20ms）。
   所以"少起子进程"优先于"换更便宜的读法"或"加并发"：`mark_eol_only` 只在
   确实可能存在纯行尾变更时才发第二次探针；`conflict_list`（一次 `ls-files -u`
   约 80ms）只在 status 已含冲突条目时才请求；refs 五连（tags / remotes /
   remote-branches / stash / reflog）只在事件种类涉及 head/refs/config/
   merge_state 时重读（`RepoTab.refsEpoch`），纯 worktree/index 事件不碰它。
   进程创建本身（`core/proctree.rs`）同理：恢复 `CREATE_SUSPENDED` 子进程
   **必须**用 `NtResumeProcess`，不能用 `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD)`
   ——后者为了一个已知 pid 去扫全系统线程表，且在全局快照锁下串行（实测每次
   spawn 白花 40ms，五个并发时每个涨到 400ms+，并行反而更慢）。也不要换成
   `DETACHED_PROCESS`：省不了时间（开销全在恢复那步），却会让 `git.cmd` 这类
   批处理 wrapper 里的 `set VAR=…` 失效（`tests/config_view.rs` 立刻变红，
   `proctree::batch_wrapper_propagates_env` 专门守这两点）。
4. **写操作持 per-repo `WriteGate`，读操作取 `read_permit()`**（并发 ≤ 8），
   照 commands/mod.rs 现有模式。
5. **AppError** 序列化为 `{ "code": "<variant>", ...fields }`（serde tag），前端
   `normalizeError` 依赖此形状。刻意不用 thiserror derive（手写 Display）——别"修复"。
6. **bindings.ts 由 tauri-specta 生成**（ADR-009）：新增命令 = `#[tauri::command]`
   + `#[specta::specta]` → 登记 `lib.rs` 的 `specta_builder()` → `cargo test` 重新生成并提交。
   不要用 `tauri::generate_handler!`，`ErrorHandlingMode::Throw` 不改。
7. **Svelte 5 runes**（ADR-002）：新代码一律 `$state/$derived/$effect`，不写
   legacy `export let` / store 订阅（toast.ts、refbus.ts 为历史遗留）。UI 基于 shadcn-svelte。
8. **用户可见字符串一律走 `t()`**（ADR-011 自研 i18n，勿引入 svelte-i18n）；
   新字符串同时补 zh-CN.json 和 en.json。
9. **功能与键位单一数据源**：可从菜单/命令面板/快捷键触达的功能登记进
   `features.ts`，键位引用 `keymap.ts`，不建第二份数据源。

## 测试约定

- 解析器：纯函数 + 内联 fixture 单测。注意 Rust `\`+换行续行吞前导空白，
  diff 上下文行前导空格写 `\x20`。
- 时序逻辑：`#[tokio::test(start_paused = true)]` + `tokio::time::advance`。
- 集成测试在 `src-tauri/tests/`：临时目录 + `git init`，超时 ≥10s；临时文件
  命名带 pid + 纳秒时间戳防并行冲突。
- 改了命令/事件/DTO 后提交重新生成的 bindings.ts。

## CI（.github/workflows/ci.yml）

- Frontend（ubuntu）：`pnpm check` + `pnpm test:unit` + `pnpm build` + 包体积报告
- Rust（windows/macos/ubuntu 矩阵）：`fmt --check` + `clippy -D warnings` + `cargo test`

## 其他注意

- 仓库用 LF（.gitattributes），Windows 下也保持 LF。
- 依赖钉子（Cargo.toml 有注释，勿清理）：keyring 必须带平台 features
  （缺了静默降级 mock，凭据悄悄失效）；rand 钉 0.8（ssh-key 0.6）；reqwest 用 rustls。
- `typescript@^6` + `@typescript/native`（npm:typescript@^7）是 svelte-check 要求，升级先跑 `pnpm check`。
- 双 bin 目标：Cargo.toml 的 `default-run = "ibexgit"` 不能删。
- Windows 坑：build.rs 为 test 目标嵌入 tests.manifest（缺了会
  STATUS_ENTRYPOINT_NOT_FOUND），别删。
- RepoId 是 worktree 路径的 FNV-1a 哈希，字符串序列化（u64 超 JS 安全整数）。
- 提交信息：阶段性用 `<阶段>: <主题>`（如 `P11: …`），日常用 `feat:`/`fix:` 等前缀，
  主题简洁中文，一个逻辑单元一个提交。
