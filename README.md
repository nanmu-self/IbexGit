<div align="center">

<img src="app-icon.png" width="96" alt="IbexGit icon" />

# IbexGit

**本地优先的 Git 图形客户端**

Tauri 2 · SvelteKit 5 (Svelte 5 runes) · TypeScript · Rust

[![CI](https://github.com/nanmu-self/IbexGit/actions/workflows/ci.yml/badge.svg)](https://github.com/nanmu-self/IbexGit/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](#license)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)](#开发环境)

</div>

---

IbexGit 是一款对齐 Fork / GitButler / SmartGit 核心工作流的开源 Git 桌面客户端：

- **快** — 大仓库（万级文件、十万级提交）全链路虚拟滚动，status/log 分批加载，diff 按需重取；
- **全** — 覆盖日常 Git 工作流 95% 场景，尽量少让用户开终端；
- **稳** — 危险操作（丢弃 / Reset / Clean / 变基 / 强推）全部有确认与撤销安全网，绝不损坏用户仓库；
- **跨平台** — Windows（首发）、macOS、Linux。

底层直接驱动**系统 git CLI**（而非 libgit2），行为与命令行一致；所有 git 子进程永不交互（`GIT_TERMINAL_PROMPT=0` 兜底），凭据走独立 helper + OS keychain，不落明文。

## 当前状态

> ⚠️ 项目处于活跃开发期（v0.1.0，内部 alpha）。核心功能已齐备但尚未提供正式安装包，欢迎 clone 源码体验 / 参与共建。

开发按 [docs/PLAN.md](docs/PLAN.md) 的 P0–P12 阶段推进，当前进度：

| 阶段 | 内容 | 状态 |
|------|------|------|
| P0 | 架构与基建（ADR、CI、SPA 骨架） | ✅ |
| P1 | Git 引擎核心层（trait / runner / watcher / porcelain 解析） | ✅ |
| P2 | 应用骨架 UI（多仓库 Tab、欢迎页、i18n、快捷键抽象） | ✅ |
| P3 + P3.5 | 工作区 + 提交 MVP；仓库分组与彩色书签 | ✅ |
| P4 | 差异审阅（统一/双栏、行级暂存、图片 diff） | ✅ |
| P5 | 提交历史（SVG 提交图、搜索筛选、多选操作） | ✅ |
| P6 | 分支 / 远程 / 标签 / 贮藏 / Reset / Reflog / Clean | ✅ |
| P7 | 克隆与凭据（CredentialBroker、keychain、SSH、代理） | ✅ |
| P8 | 冲突解决（CodeMirror 编辑器、四类冲突、崩溃恢复） | ✅ |
| P9 | 文件追溯（单文件历史 --follow、Blame） | ✅ |
| P10 | 菜单 / 快捷键 / 命令面板 / 设置中心 | ✅ |
| P11 | AI 助手（智能提交消息 + 日报/周报，BYOK） | ✅ |
| P12 | 打磨与发布（性能审计、自动更新、多平台打包分发） | 🚧 进行中 |

## 功能特性

**仓库管理**
- 多仓库 Tab 同时打开，各自保留分支、暂存与视图状态；拖拽文件夹 / 最近仓库快速接入
- 仓库总览页：搜索、分组管理、彩色书签
- 新建空仓库、克隆 HTTP / SSH 远程（深度 / 单分支 / 递归子模块，实时进度）
- FS watcher 自动刷新：外部工具改动仓库后自动失效重读，免手动刷新

**工作区与提交**
- 未暂存 / 已暂存 / 冲突三分区，列表 ↔ 文件树切换，批量暂存、Ctrl/Shift 连选
- 丢弃自动前置快照，toast 一键撤销；行级暂存与行级丢弃（`git apply`）
- 提交框：Amend、跳过钩子、提交并推送、50/72 提示、commit template、Co-authored-by 尾注
- 子模块状态显示、CRLF/EOL 特殊状态标注

**差异审阅**
- 统一 / 双栏视图，Shiki 语法高亮（本地分包，按需加载）
- 未变更块折叠 + 展开上下文（`--unified=N` 按需重取）、词级差异高亮、空白显示开关
- 图片 diff：并排 / 滑动对比 / 差异叠加；重命名检测（旧路径划线 → 新路径 + 相似度）

**历史与引用**
- SVG 可视化提交图（Rust 拓扑 + lane 布局，分批增量加载），节点 / 边 / HEAD / 标签徽章
- 搜索（消息 / 哈希 / 作者）与筛选（作者 / 日期 / 路径）；分支比较（ahead/behind + 双向 diff）
- 多选 Cherry-pick / Squash / Revert（预览对话框 + 进度）；HEAD 分离状态引导
- 分支检出 / 新建 / 删除 / 重命名，Merge / Rebase（dry-run 预览），`--force-with-lease` 推送
- 标签（附注 / 轻量）、贮藏（apply / pop / drop / 查看 diff）、Reset 三档、Clean 预览、Reflog 浏览器

**冲突解决**
- merge / rebase / cherry-pick / pull 四类冲突全流程闭环；Content / Delete-Modify / Add-Add / Binary 四类可视化解决
- CodeMirror 6 冲突编辑器（按需动态加载）、接受当前 / 传入 / 双方、冲突间导航
- 外部 mergetool（VSCode / Meld / KDiff3 / P4Merge / Vimdiff / 自定义）；中途杀进程重开可恢复

**文件追溯**
- 单文件历史窗口（`log --follow` 重命名追踪、游标分页）
- Blame 按行审阅（按提交着色分组），点击行跳转对应提交 diff

**效率与个性化**
- 全功能菜单栏 + 全局快捷键体系 + 命令面板（Ctrl+Shift+P，模糊搜索 + 最近使用）
- 设置中心：主题、语言（中/英）、Git 路径与 pull 策略、网络（代理 / SSH key）、凭据管理、SSH 密钥生成、日志级别热更、Git 配置查看器
- AI 助手（自带 Key）：智能提交消息（diff → 消息，可重新生成多方案）、日报 / 周报生成（跨仓库聚合、map-reduce）；支持 OpenAI 兼容端点 / Anthropic / Ollama（本地离线），隐私排除规则 + 发送前预览，**API Key 只存 OS keychain，绝不进 WebView**

## 技术栈与架构

| 层 | 选型 |
|----|------|
| 桌面框架 | Tauri 2（Rust 后端 + 系统 WebView） |
| 前端 | SvelteKit 5 SPA（Svelte 5 runes）+ Tailwind 4 + shadcn-svelte |
| 语法高亮 / 编辑器 | Shiki（本地分包）· CodeMirror 6（冲突场景动态加载） |
| Git 引擎 | 系统 git CLI（≥ 2.40）+ `--porcelain` 自写解析器 |
| 凭据存储 | OS keychain（Windows 凭据管理器 / macOS Keychain / Linux keyutils） |
| 类型同步 | tauri-specta：`#[tauri::command]` → 生成 TS 绑定（编译期对齐） |
| 日志 | tracing + 按天滚动文件日志 |

标准调用链：**UI → Task → Operation → RepoQueue（每仓库串行写）→ GitEngine → CliEngine → GitProcessRunner → git**。

几条核心设计（详见 [docs/PLAN.md](docs/PLAN.md) 与 [docs/adr/](docs/adr/)）：

- **GitEngine trait 是 git 操作唯一入口**（[ADR-001](docs/adr/ADR-001-git-cli-over-libgit2.md)）：新能力 = trait 方法 + CLI 实现 + 纯函数解析器 + fixture 单测；
- **缓存只作展示加速，`.git` 与工作区是唯一真相源**：所有变更（含应用自身写操作）统一走 watcher 失效路径 → `repo://changed` 事件；
- **并发模型**（[ADR-005](docs/adr/ADR-005-repo-concurrency-model.md)）：写操作持 per-repo WriteGate 串行执行规避 index.lock 冲突，只读操作全局并发 ≤ 8；
- **恢复体系**：丢弃 / 重置 / 变基 / 合并均有撤销安全网（工作区快照 + backup ref 双轨）。

完整架构决策记录见 [docs/adr/](docs/adr/)（git CLI 选型、diff 管线、凭据 broker、虚拟渲染、包体策略、AI Provider 管线等 14 篇）。

## 开发环境

要求：**Node 22+ / pnpm 11+ / Rust stable**，且 `git`（≥ 2.40）在 PATH 中（部分集成测试与运行时均依赖真实 git）。

```bash
# 安装前端依赖
pnpm install

# 桌面应用开发模式（自动拉起前端 dev server）
pnpm tauri dev

# 前端开发
pnpm dev
```

### 常用命令

```bash
pnpm check                # svelte-check 类型检查（必须 0 error）
pnpm build                # 前端构建（adapter-static）

cd src-tauri
cargo fmt --all           # 格式化
cargo clippy --all-targets -- -D warnings   # lint（0 warning）
cargo test                # 全部测试；tauri-specta 绑定在此时重新生成
```

提交前全套：`cargo fmt` → `cargo clippy -- -D warnings` → `cargo test` → `pnpm check` → `pnpm build`。

> 注意：`src/lib/git/bindings.ts` 由 tauri-specta 生成，勿手改；改动命令 / 事件 / DTO 后 `cargo test` 重新生成并一并提交。

### 目录结构

```
src/                        # SvelteKit 前端
  lib/git/                  # 类型化命令 API、repo-changed 事件、错误归一化
  lib/git/bindings.ts       # tauri-specta 生成（勿手改）
  lib/components/           # ui / workspace / diff / history / refs / merge / settings / ai …
  lib/{keyboard,stores,i18n,theme}
src-tauri/src/
  core/
    engine/                 # GitEngine trait、porcelain 解析器、CLI 实现
    runner.rs               # GitProcessRunner：spawn/超时/kill/stdin 模式
    repo.rs                 # RepoManager：会话缓存/写 gate/读信号量
    watcher.rs              # notify → 分类 → 防抖 → 失效事件
    task.rs / graph.rs / recovery.rs / credential.rs / ai/ …
  commands/                 # #[tauri::command] 薄封装
docs/
  PLAN.md                   # 开发计划（P0–P12，进度真相源）
  adr/                      # 架构决策记录
  capability-matrix.md      # Git 能力规格与 Feature 分层
  design/                   # 专项设计（凭据/SSH、键位表）
```

### 测试约定

- porcelain 解析器为纯函数 + 内联 fixture 单测，另有 Golden Test 固定期望输出；
- 时序逻辑（防抖等）用 `tokio::test(start_paused = true)`；
- 真实 git 集成测试（watcher 端到端、并发 stage、冲突恢复、AI 流程等）：临时目录 + `git init`。

CI（GitHub Actions）在每 PR 上运行前端 `check + build`，并在 Windows / macOS / Ubuntu 三平台矩阵上运行 `cargo fmt --check` + `clippy -D warnings` + `cargo test`。

## 文档

- [docs/PLAN.md](docs/PLAN.md) — 产品定位、功能清单、总体架构、阶段计划（P0–P12）、难点预案
- [docs/adr/](docs/adr/) — 架构决策记录（14 篇）
- [docs/capability-matrix.md](docs/capability-matrix.md) — Git 能力规格（Capability）与 Feature 分层
- [AGENTS.md](AGENTS.md) — AI 编码代理协作约定

## License

MIT（仓库尚未添加 LICENSE 文件，待 P12 发布阶段补齐）
