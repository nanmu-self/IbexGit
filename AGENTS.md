# AGENTS.md — AI 编码代理指南

IbexGit 是一个**本地优先的 Git 图形客户端**（Tauri 2 + SvelteKit 5 + TypeScript + Rust）。
本文件给 AI 代理提供高效工作所需的约定与红线。详细规划见 `docs/PLAN.md`（P0–P12 阶段），
架构决策见 `docs/adr/`。

## 常用命令

```bash
pnpm install              # 安装前端依赖（用 pnpm，CI 用 frozen-lockfile）
pnpm check                # svelte-check 类型检查（必须 0 error）
pnpm build                # 前端构建（adapter-static）
pnpm dev                  # 前端 dev server

cd src-tauri
cargo fmt --all           # 格式化（提交前必跑，CI 用 --check）
cargo clippy --all-targets -- -D warnings   # lint（0 warning）
cargo test                # 全部测试；ts-rs 绑定在此时重新生成
pnpm tauri dev            # 桌面应用开发模式
```

提交前全套：`cargo fmt` → `cargo clippy -- -D warnings` → `cargo test` → `pnpm check` → `pnpm build`。
测试需要 `git` 在 PATH 中（部分集成测试用真实 git 仓库）。

## 目录结构（PLAN §5）

```
src/                        # SvelteKit 前端
  lib/git/                  # invoke 封装、类型化命令 API、repo://changed 事件
  lib/git/bindings/         # ts-rs 生成（勿手改）
  lib/components/{ui,workspace,diff,history,refs,merge}
  lib/{keyboard,stores,i18n,theme}
src-tauri/src/
  core/
    engine/{mod,parse,cli}.rs   # GitEngine trait（唯一入口）、解析器、CLI 实现
    runner.rs                   # GitProcessRunner：spawn/超时/kill/stdin 模式
    repo.rs                     # RepoManager：打开/会话缓存/写 gate/读信号量
    watcher.rs                  # 状态失效：notify → 分类 → 防抖 → 事件
    task.rs / graph.rs / recovery.rs / credential.rs / compat.rs / error.rs
  commands/mod.rs           # #[tauri::command] 薄封装
docs/{PLAN.md,adr/,capability-matrix.md,design/}
```

## 架构红线（改动前必读）

1. **GitEngine trait 是 git 操作唯一入口**（ADR-001：git CLI 而非 libgit2）。
   不要在 runner.rs 之外直接 `Command::new("git")`；新 git 能力 =
   trait 方法 + cli.rs 实现 + parse.rs 纯函数解析器 + 单测。

2. **git 子进程永不交互**：stdin 只允许 `Null`（默认，读即快速失败）或
   `Feed`（一次性写入后关闭）。环境兜底 `GIT_TERMINAL_PROMPT=0`、
   `GIT_EDITOR=true`，并强制 `-c core.quotepath=false`（中文路径 raw UTF-8）。
   凭据将来走独立 helper 子进程，不经 git 主进程 stdin。

3. **缓存只作展示加速，`.git` 与工作区是唯一真相源**（§4.3）。
   RepoManager 不提供"长期可信"状态；任何变更（含 IbexGit 自己的写操作）
   都必须走 watcher 失效路径 → `invalidate()` → `repo://changed`。
   不要为内部写操作另开快速通道。

4. **变更类操作必须持有 per-repo `WriteGate`**（避免 index.lock 冲突）；
   只读操作先取 `read_permit()`（全局并发 ≤ 8）。见 commands/mod.rs 现有
   模式，新命令照抄。

5. **AppError 序列化为 `{ "code": "<variant>", ...fields }`**（serde tag）。
   前端 `normalizeError`（lib/git/index.ts）依赖此形状。注意：AppError 刻意
   **不用** `thiserror::Error` derive（String source 与 AsDynError 冲突），
   手写 `Display` + `std::error::Error`——别"修复"回去。

6. **`src/lib/git/bindings/` 由 ts-rs 生成**。改类型 = 改 Rust 结构体 +
   `cargo test` 重新生成 + 提交产物。不要手改生成文件。

7. **Svelte 5 runes**（ADR-002）：新组件用 `$state/$derived/$effect`，
   不要引入 legacy `export let` / store 订阅语法（stores/ 目录的既有
   writable 除外）。UI 组件基于 shadcn-svelte（ADR-008）。

8. **i18n**：P2 起用户可见字符串一律走 `lib/i18n` 的 `t()`，不要硬编码。

## 测试约定

- **解析器**：纯函数 + 内联 fixture 单测。注意 Rust 字符串续行 `\`+换行
  会吞掉下一行前导空白——unified diff 上下文行的前导空格要写 `\x20`。
- **时序逻辑**（防抖等）：`#[tokio::test(start_paused = true)]` +
  `tokio::time::advance`（tokio dev-dep 含 test-util）。
- **真实 git 集成测试**（watcher 端到端、并发 stage、engine smoke 闭环）：
  临时目录 + `git init`，超时给足（≥10s）。CI 三平台都装了 git。
- ts-rs 的导出测试在 `cargo test` 中运行；改了 Rust DTO 后记得提交
  重新生成的 bindings。
- 新增解析器/防抖/缓存行为必须有对应单测——这是 P1 的验收标准之一。

## CI（.github/workflows/ci.yml）

- Frontend（ubuntu，Node 22 + pnpm 11）：`pnpm check` + `pnpm build` + 包体积报告
- Rust（windows / macos / ubuntu 矩阵）：`cargo fmt --check` + `clippy -D warnings` + `cargo test`

## 其他注意事项

- **版本钉子**：`typescript@^6` + `@typescript/native`（npm:typescript@^7）
  是 svelte-check 4.7 的要求，升级前先跑 `pnpm check` 验证。
- 仓库用 LF（.gitattributes）；Windows 下开发也保持 LF。
- 类型同步现状：P1 用 **ts-rs**（仅类型，产物入库），与 ADR-009 的
  tauri-specta 决策有偏离——偏离理由见 ADR-009 附录，不要单方面切回。
- 提交信息风格：`<阶段>: <主题>`（如 `P1: ...`），正文列要点；
  一个逻辑单元一个提交。
- PLAN.md 的阶段清单是进度真相源：完成一项勾一项（`[ ]` → `[x]`）。
