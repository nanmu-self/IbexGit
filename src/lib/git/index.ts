import { commands, type RepoId } from "./bindings";
import { addToast } from "$lib/stores/toast";
import type { AppError } from "$lib/stores/toast";
import type { RepoUiState } from "./bindings";

// =====================
// Types — generated from Rust by tauri-specta (ADR-009). `cargo test`
// regenerates src/lib/git/bindings.ts; edit the Rust structs, not this file.
// =====================

export { commands };

export type {
  AppError,
  BranchInfo,
  CommitInfo,
  CommitResult,
  DiffFile,
  DiffHunk,
  DiffLine,
  DiffLineKind,
  DiffModel,
  DiffSource,
  FileStatus,
  RecentRepo,
  RecoveryEntry,
  RepoId,
  RepoUiState,
  SelectedFile,
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
    paths?: string[]
  ) => wrap(commands.gitDiff(id, source, oldRev ?? null, newRev ?? null, paths ?? null)),
  branches: (id: RepoId) => wrap(commands.gitBranches(id)),
  checkoutBranch: (id: RepoId, name: string) =>
    wrap(commands.gitCheckoutBranch(id, name)),
};

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
 * Workspace persistence (PLAN §4.4): recent repositories + per-repo UI
 * state under `{appData}/workspaces/`.
 */
export const workspace = {
  recents: () => wrap(commands.workspaceRecents()),
  touchRecent: (path: string, name: string) =>
    wrap(commands.workspaceTouchRecent(path, name)),
  forgetRecent: (path: string) => wrap(commands.workspaceForgetRecent(path)),
  loadState: (repoPath: string) => wrap(commands.workspaceLoadState(repoPath)),
  saveState: (repoPath: string, state: RepoUiState) =>
    wrap(commands.workspaceSaveState(repoPath, state)),
};
