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
3. **缓存只作展示加速，`.git` 与工作区是唯一真相源**。所有变更（含自身写操作）
   走 watcher 失效 → `repo://changed`，不为内部写操作开快速通道。
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

- Frontend（ubuntu）：`pnpm check` + `pnpm build` + 包体积报告
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
