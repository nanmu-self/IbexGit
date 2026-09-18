import { commands, type RepoId, type SshKeyGenerateRequest } from "./bindings";
import { t } from "$lib/i18n";
import type {
  AiConfigDto,
  AiPreview,
  AiReportRequest,
  CloneRequest,
  CredentialReply,
  GraphFilter,
  LineSelection,
  NetConfig,
} from "./bindings";
import { addToast } from "$lib/stores/toast";
import type { AppError } from "$lib/stores/toast";
import type { RepoMeta, RepoUiState } from "./bindings";

// =====================
// Types — generated from Rust by tauri-specta (ADR-009). `cargo test`
// regenerates src/lib/git/bindings.ts; edit the Rust structs, not this file.
// =====================

export { commands };

export type {
  AiConfigDto,
  AiEvent,
  AiPreview,
  AiReportRequest,
  AppError,
  BackupRef,
  BlameCommit,
  BlameLine,
  BlameResult,
  BranchCompare,
  BranchInfo,
  CloneEvent,
  CloneRequest,
  CommitDetail,
  CommitFileStat,
  CommitInfo,
  CommitResult,
  CommitTemplate,
  ConfigEntry,
  ConflictBlock,
  ConflictModel,
  ConflictSummary,
  ConflictType,
  CredentialEntry,
  CredentialPrompt,
  CredentialReply,
  DiffFile,
  DiffHunk,
  DiffLine,
  DiffLineKind,
  DiffModel,
  DiffSource,
  FileCommit,
  FileContent,
  FileStatus,
  GitignoreFile,
  GraphEdge,
  GraphFilter,
  GraphPage,
  GraphRow,
  GroupsFile,
  KnownHost,
  LineSelection,
  NetConfig,
  OperationKind,
  OperationState,
  RecentRepo,
  RecoveryEntry,
  ReflogEntry,
  RemoteInfo,
  RepoConfigValue,
  RepoGroup,
  RepoId,
  RepoMeta,
  RepoUiState,
  ResetUndo,
  SelectedFile,
  SshKeyAlgorithm,
  SshKeyGenerateRequest,
  SshKeyInfo,
  StashEntry,
  TagInfo,
} from "./bindings";

/**
 * Unified error pipeline: tauri-specta commands reject with the serialized
 * `AppError` (serde `tag = "code"`); normalize it and surface a toast.
 */
export function normalizeError(raw: unknown): AppError {
  if (typeof raw === "string") {
    return { code: "internal", message: raw, detail: null };
  }
  const e = (raw ?? {}) as Record<string, unknown>;
  const code = typeof e.code === "string" ? e.code : "internal";
  const message = humanize(code, e);
  const detail = typeof e.detail === "string" ? e.detail : null;
  const appError: AppError = { code, message, detail };
  addToast(appError);
  return appError;
}

function humanize(code: string, e: Record<string, unknown>): string {
  switch (code) {
    case "io":
      return str(e.source) || "I/O error";
    case "git_command":
      return str(e.stderr) || `git failed: ${str(e.command) || "?"}`;
    case "git_version_too_old":
      return `Git ${str(e.found)} is too old (need ${str(e.required)})`;
    case "invalid_repo":
      return `Not a git repository: ${str(e.path)}`;
    case "operation_cancelled":
      return "Operation cancelled";
    case "credential_cancelled":
      return "Credential prompt cancelled";
    case "diff_model_expired":
      return "Diff expired (repository changed) — please retry";
    case "ai_disabled":
      return t("ai.error.disabled");
    case "ai_config":
      return `${t("ai.error.config")}: ${str(e.message) ?? ""}`.trim();
    case "ai_network":
      return `${t("ai.error.network")} (${str(e.message) ?? ""})`;
    case "ai_auth":
      return `${t("ai.error.auth")} (${str(e.message) ?? ""})`;
    case "ai_rate_limit":
      return t("ai.error.rateLimit");
    case "ai_provider":
      return `${t("ai.error.provider")} (${str(e.message) ?? ""})`;
    case "parse":
      return str(e.message) || "Failed to parse git output";
    case "not_implemented":
      return `Not yet implemented: ${str(e.feature)}`;
    default:
      return str(e.message) || "Unknown error";
  }
}

function str(v: unknown): string | null {
  return typeof v === "string" && v.length > 0 ? v : null;
}

async function wrap<T>(p: Promise<T>): Promise<T> {
  try {
    return await p;
  } catch (raw) {
    throw normalizeError(raw);
  }
}

// =====================
// Typed command API (wraps generated bindings with the toast pipeline)
// =====================

export const repo = {
  open: (path: string) => wrap(commands.repoOpen(path)),
  close: (id: RepoId) => wrap(commands.repoClose(id)),
  list: () => wrap(commands.repoList()),
};

export const git = {
  status: (id: RepoId) => wrap(commands.gitStatus(id)),
  stage: (id: RepoId, paths: string[]) => wrap(commands.gitStage(id, paths)),
  unstage: (id: RepoId, paths: string[]) =>
    wrap(commands.gitUnstage(id, paths)),
  /** Discard with a recovery snapshot; resolves to the snapshot id for undo. */
  discard: (id: RepoId, paths: string[], scope: "worktree" | "all" = "worktree") =>
    wrap(commands.gitDiscard(id, paths, scope)),
  commit: (id: RepoId, message: string, amend = false, noVerify = false) =>
    wrap(commands.gitCommit(id, message, amend, noVerify)),
  headMessage: (id: RepoId) => wrap(commands.gitHeadMessage(id)),
  ignorePaths: (id: RepoId, paths: string[]) =>
    wrap(commands.gitIgnorePaths(id, paths)),
  log: (id: RepoId, limit = 200, offset = 0, paths?: string[]) =>
    wrap(commands.gitLog(id, limit, offset, paths ?? null)),
  diff: (
    id: RepoId,
    source: "worktree" | "staged" | "commit" | "stash",
    oldRev?: string,
    newRev?: string,
    paths?: string[],
    contextLines?: number,
    ignoreWhitespace?: boolean
  ) =>
    wrap(
      commands.gitDiff(
        id,
        source,
        oldRev ?? null,
        newRev ?? null,
        paths ?? null,
        contextLines ?? null,
        ignoreWhitespace ?? null
      )
    ),
  /** Line-level stage (`git apply --cached`) against a cached DiffModel. */
  stageLines: (id: RepoId, modelId: number, path: string, selections: LineSelection[]) =>
    wrap(commands.gitStageLines(id, modelId, path, selections)),
  /** Line-level discard; resolves to the recovery snapshot id for undo. */
  discardLines: (id: RepoId, modelId: number, path: string, selections: LineSelection[]) =>
    wrap(commands.gitDiscardLines(id, modelId, path, selections)),
  /** Line-level unstage (`git apply --cached --reverse`). */
  unstageLines: (id: RepoId, modelId: number, path: string, selections: LineSelection[]) =>
    wrap(commands.gitUnstageLines(id, modelId, path, selections)),
  /** Content of one revision of a file (image diff); null data = too large. */
  fileContent: (id: RepoId, path: string, rev: string | null) =>
    wrap(commands.gitFileContent(id, path, rev)),

  // ---- file trace (P9 单文件历史 + Blame) ----
  /** Single-file history with rename following, newest first.
   *  `start` = last hash of the previous page (cursor pagination). */
  fileHistory: (id: RepoId, path: string, limit = 500, start?: string | null) =>
    wrap(commands.gitFileHistory(id, path, limit, start ?? null)),
  /** Blame the current worktree version of a path. */
  blame: (id: RepoId, path: string) => wrap(commands.gitBlame(id, path)),
  branches: (id: RepoId) => wrap(commands.gitBranches(id)),
  checkoutBranch: (id: RepoId, name: string) =>
    wrap(commands.gitCheckoutBranch(id, name)),
  createBranch: (id: RepoId, name: string, startPoint?: string) =>
    wrap(commands.gitCreateBranch(id, name, startPoint ?? null)),
  deleteBranch: (id: RepoId, name: string, force = false) =>
    wrap(commands.gitDeleteBranch(id, name, force)),
  renameBranch: (id: RepoId, oldName: string, newName: string) =>
    wrap(commands.gitRenameBranch(id, oldName, newName)),
  /** Set (or clear with null) the upstream tracking of a branch. */
  setUpstream: (id: RepoId, branch: string, upstream: string | null) =>
    wrap(commands.gitSetUpstream(id, branch, upstream)),

  // ---- tags (P6) ----
  tags: (id: RepoId) => wrap(commands.gitTags(id)),
  createTag: (id: RepoId, name: string, message: string | null, target: string) =>
    wrap(commands.gitCreateTag(id, name, message, target)),
  deleteTag: (id: RepoId, name: string) => wrap(commands.gitDeleteTag(id, name)),

  // ---- stash (P6) ----
  stashList: (id: RepoId) => wrap(commands.gitStashList(id)),
  stashPush: (id: RepoId, message: string | null) =>
    wrap(commands.gitStashPush(id, message)),
  stashApply: (id: RepoId, index: number) => wrap(commands.gitStashApply(id, index)),
  stashPop: (id: RepoId, index: number) => wrap(commands.gitStashPop(id, index)),
  stashDrop: (id: RepoId, index: number) => wrap(commands.gitStashDrop(id, index)),

  // ---- remotes (P6) ----
  remotes: (id: RepoId) => wrap(commands.gitRemotes(id)),
  addRemote: (id: RepoId, name: string, url: string) =>
    wrap(commands.gitAddRemote(id, name, url)),
  removeRemote: (id: RepoId, name: string) => wrap(commands.gitRemoveRemote(id, name)),
  setRemoteUrl: (id: RepoId, name: string, url: string, push = false) =>
    wrap(commands.gitSetRemoteUrl(id, name, url, push)),
  pruneRemote: (id: RepoId, name: string) => wrap(commands.gitPruneRemote(id, name)),

  // ---- network / history ops (P6) ----
  fetch: (id: RepoId, remote?: string | null) =>
    wrap(commands.gitFetch(id, remote ?? null)),
  /** mode: "merge" | "rebase" | "ff_only". */
  pull: (id: RepoId, remote: string | null, branch: string | null, mode: string | null) =>
    wrap(commands.gitPull(id, remote, branch, mode)),
  push: (
    id: RepoId,
    remote: string,
    branch: string,
    forceWithLease = false,
    setUpstream = false,
    tags = false
  ) => wrap(commands.gitPush(id, remote, branch, forceWithLease, setUpstream, tags)),
  merge: (id: RepoId, target: string, ffOnly = false) =>
    wrap(commands.gitMerge(id, target, ffOnly)),
  rebase: (id: RepoId, target: string) => wrap(commands.gitRebase(id, target)),

  // ---- reset with recovery + undo (P6) ----
  /** soft | mixed | hard; resolves to the undo anchors. */
  reset: (id: RepoId, mode: string, target: string) =>
    wrap(commands.gitReset(id, mode, target)),
  undoReset: (id: RepoId, backupRef: string, mode: string, snapshotId: string | null) =>
    wrap(commands.gitUndoReset(id, backupRef, mode, snapshotId)),

  // ---- clean (P6): preview → per-item confirm → delete ----
  cleanList: (id: RepoId) => wrap(commands.gitCleanList(id)),
  /** Resolves to the recovery snapshot id for undo. */
  clean: (id: RepoId, paths: string[]) => wrap(commands.gitClean(id, paths)),

  // ---- reflog browser (P6) ----
  reflog: (id: RepoId, refName?: string | null) =>
    wrap(commands.gitReflog(id, refName ?? null)),

  // ---- branch compare + previews (P6) ----
  branchCompare: (id: RepoId, left: string, right: string) =>
    wrap(commands.gitBranchCompare(id, left, right)),
  /** Commits in a rev range (e.g. "HEAD..origin/main"), newest first. */
  revList: (id: RepoId, range: string, limit = 200, offset = 0) =>
    wrap(commands.gitRevList(id, range, limit, offset)),

  // ---- backup refs (P6 孤儿备份清理) ----
  backupList: (id: RepoId) => wrap(commands.gitBackupList(id)),
  backupDelete: (id: RepoId, names: string[]) =>
    wrap(commands.gitBackupDelete(id, names)),

  // ---- history / commit graph (P5) ----
  /** First page of the laid-out commit graph (500 rows per batch). */
  graph: (id: RepoId, filter?: GraphFilter | null) =>
    wrap(commands.gitGraph(id, filter ?? null)),
  /** Next graph page; `start === 0` in the response means "replace list". */
  graphMore: (id: RepoId, filter?: GraphFilter | null) =>
    wrap(commands.gitGraphMore(id, filter ?? null)),
  /** Commit metadata + changed files. */
  commitDetail: (id: RepoId, hash: string) =>
    wrap(commands.gitCommitDetail(id, hash)),
  /** Cherry-pick commits onto HEAD, applied in list order (oldest first). */
  cherryPick: (id: RepoId, hashes: string[]) =>
    wrap(commands.gitCherryPick(id, hashes)),
  /** Revert commits (newest first), one inverse commit each. */
  revert: (id: RepoId, hashes: string[]) =>
    wrap(commands.gitRevert(id, hashes)),
  /** Squash the newest consecutive commits into one with `message`. */
  squash: (id: RepoId, hashes: string[], message: string) =>
    wrap(commands.gitSquash(id, hashes, message)),
  /** Restore file(s) from a revision into the worktree (snapshot-backed). */
  restoreFileVersion: (id: RepoId, rev: string, paths: string[]) =>
    wrap(commands.gitRestoreFileVersion(id, rev, paths)),

  // ---- conflicts & operation state (P8) ----
  /** All conflicted paths with lightweight classification. */
  conflictList: (id: RepoId) => wrap(commands.gitConflictList(id)),
  /** Full model for one conflicted path (editor input). */
  conflictModel: (id: RepoId, path: string) =>
    wrap(commands.gitConflictModel(id, path)),
  /** Write the resolved document back and stage the path. */
  resolveConflictText: (id: RepoId, path: string, text: string) =>
    wrap(commands.gitResolveConflictText(id, path, text)),
  /** Resolve by side: "ours" | "theirs" | "delete". */
  resolveConflictSide: (id: RepoId, path: string, action: "ours" | "theirs" | "delete") =>
    wrap(commands.gitResolveConflictSide(id, path, action)),
  /** In-progress operation (merge/rebase/cherry-pick/…), if any. */
  operationState: (id: RepoId) => wrap(commands.gitOperationState(id)),
  operationAbort: (id: RepoId) => wrap(commands.gitOperationAbort(id)),
  operationContinue: (id: RepoId) => wrap(commands.gitOperationContinue(id)),
  operationSkip: (id: RepoId) => wrap(commands.gitOperationSkip(id)),
  /** Open an external merge tool (git built-in name or custom command). */
  mergetool: (id: RepoId, path: string, tool?: string, cmd?: string) =>
    wrap(commands.gitMergetool(id, path, tool ?? null, cmd ?? null)),
};

/**
 * SHA-1 of git's empty tree — parent stand-in for root-commit diffs
 * (`git diff <empty-tree> <root>` equals the root commit's diff).
 */
export const EMPTY_TREE_SHA = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/**
 * DiscardRecovery (PLAN §4.7 轨道 A): snapshot list / restore / delete.
 * Restore triggers the normal watcher→refresh pipeline on completion.
 */
export const recovery = {
  list: (id: RepoId) => wrap(commands.recoveryList(id)),
  restore: (id: RepoId, snapshotId: string) =>
    wrap(commands.recoveryRestore(id, snapshotId)),
  remove: (id: RepoId, snapshotId: string) =>
    wrap(commands.recoveryDelete(id, snapshotId)),
};

/**
 * P7 网络/凭据：克隆（可取消 + 进度事件）、新建仓库、凭据管理、
 * host key 信任、代理与 SSH 配置。
 */
export const net = {
  /** 启动后台克隆任务，返回 taskId；进度经 onCloneEvent 事件推送。 */
  clone: (request: CloneRequest) => wrap(commands.gitClone(request)),
  cloneCancel: (taskId: number) => wrap(commands.cloneCancel(taskId)),
  /** 新建空仓库（可选 README / .gitignore 模板，不自动提交）。 */
  initRepo: (path: string, readme: boolean, gitignore: boolean) =>
    wrap(commands.repoInit(path, readme, gitignore)),

  // ---- 凭据管理（机密只存 OS keychain，索引不含明文） ----
  credentialList: () => wrap(commands.credentialList()),
  credentialDelete: (key: string) => wrap(commands.credentialDelete(key)),
  /** 回复凭据请求（三动作：submit / trust_host_key / cancel）。 */
  credentialRespond: (requestId: string, reply: CredentialReply) =>
    wrap(commands.credentialRespond(requestId, reply)),

  // ---- SSH host key 信任库 ----
  knownHostsList: () => wrap(commands.knownHostsList()),
  knownHostsRemove: (host: string) => wrap(commands.knownHostsRemove(host)),

  // ---- SSH 密钥管理（~/.ssh 列出/生成/删除，设置中心） ----
  sshKeyList: () => wrap(commands.sshKeyList()),
  sshKeyGenerate: (req: SshKeyGenerateRequest) => wrap(commands.sshKeyGenerate(req)),
  sshKeyDelete: (path: string) => wrap(commands.sshKeyDelete(path)),

  // ---- 代理 / SSH 配置（runner spawn 时统一注入） ----
  setNetConfig: (config: NetConfig) => wrap(commands.appSetNetConfig(config)),
};

/**
 * P10 应用层：Git 配置查看器、常用项编辑、仓库级常用配置、提交模板、
 * git 路径探测、日志级别。
 */
export const app = {
  configGlobal: () => wrap(commands.gitConfigGlobal()),
  configLocal: (id: RepoId) => wrap(commands.gitConfigLocal(id)),
  /** Write/unset a user-level config key (`value === null` → unset). */
  configSetGlobal: (key: string, value: string | null) =>
    wrap(commands.gitConfigSetGlobal(key, value)),
  /** Common repo config keys with local + effective values（仓库设置）。 */
  repoConfigValues: (id: RepoId) => wrap(commands.gitRepoConfigValues(id)),
  /** Write/unset a repo-local config key (`value === null` → unset). */
  repoConfigSet: (id: RepoId, key: string, value: string | null) =>
    wrap(commands.gitRepoConfigSet(id, key, value)),
  gitignoreGlobal: () => wrap(commands.gitGitignoreGlobal()),
  /** Resolved `commit.template` (path + content), or null when unset. */
  commitTemplate: (id: RepoId) => wrap(commands.gitCommitTemplate(id)),
  /** Validate a user-configured git executable; resolves to its version. */
  checkGitPath: (path: string) => wrap(commands.appCheckGitPath(path)),
  setLogLevel: (level: string) => wrap(commands.appSetLogLevel(level)),
  /** Open the OS terminal at a directory (toolbar: open in terminal). */
  openTerminal: (path: string) => wrap(commands.appOpenTerminal(path)),
  /** Open the system file manager entering a directory (toolbar). */
  openFolder: (path: string) => wrap(commands.appOpenFolder(path)),
};

/**
 * P11 AI 助手：配置（key 不出 Rust，只回 has_key）、发送前预览、
 * 生成任务（Task 化 + AiEvent 流式）、取消、导出。
 */
export const ai = {
  configGet: () => wrap(commands.aiConfigGet()),
  configSet: (config: AiConfigDto) => wrap(commands.aiConfigSet(config)),
  /** Key 只写不读；成功后前端只保留 has_key 状态。 */
  setKey: (key: string) => wrap(commands.aiSetKey(key)),
  deleteKey: () => wrap(commands.aiDeleteKey()),
  testConnection: () => wrap(commands.aiTestConnection()),
  previewCommitMessage: (id: RepoId) => wrap(commands.aiPreviewCommitMessage(id)),
  previewReport: (request: AiReportRequest) => wrap(commands.aiPreviewReport(request)),
  /** Resolves to a taskId; progress arrives via onAiEvent. */
  generateCommitMessage: (id: RepoId, language: string) =>
    wrap(commands.aiGenerateCommitMessage(id, language)),
  generateReport: (request: AiReportRequest) =>
    wrap(commands.aiGenerateReport(request)),
  cancel: (taskId: number) => wrap(commands.aiCancel(taskId)),
  exportMarkdown: (path: string, content: string) =>
    wrap(commands.aiExportMarkdown(path, content)),
};

/**
 * Workspace persistence (PLAN §4.4): recent repositories + per-repo UI
 * state under `{appData}/workspaces/`. Groups/stars (P3.5) live in the
 * same directory (`groups.json`) as the long-lived organizational layer.
 */
export const workspace = {
  recents: () => wrap(commands.workspaceRecents()),
  touchRecent: (path: string, name: string) =>
    wrap(commands.workspaceTouchRecent(path, name)),
  forgetRecent: (path: string) => wrap(commands.workspaceForgetRecent(path)),
  loadState: (repoPath: string) => wrap(commands.workspaceLoadState(repoPath)),
  saveState: (repoPath: string, state: RepoUiState) =>
    wrap(commands.workspaceSaveState(repoPath, state)),
  groups: () => wrap(commands.workspaceGroups()),
  upsertGroup: (id: string | null, name: string) =>
    wrap(commands.workspaceUpsertGroup(id, name)),
  deleteGroup: (id: string) => wrap(commands.workspaceDeleteGroup(id)),
  updateRepo: (path: string, meta: RepoMeta) =>
    wrap(commands.workspaceUpdateRepo(path, meta)),
};
