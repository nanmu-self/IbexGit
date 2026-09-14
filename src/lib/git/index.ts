import { invoke } from "@tauri-apps/api/core";
import { addToast } from "$lib/stores/toast";
import type { AppError } from "$lib/stores/toast";

// =====================
// Types mirroring Rust engine structs
// =====================

export interface FileStatus {
  path: string;
  status: string;
  orig_path?: string | null;
  submodule: boolean;
  staged: boolean;
  unstaged: boolean;
  untracked: boolean;
  skipped: boolean;
  conflict: boolean;
}

export interface CommitInfo {
  hash: string;
  short_hash: string;
  author: string;
  email: string;
  date: string;
  message: string;
  refs: string[];
  parents: string[];
}

export interface CommitResult {
  hash: string;
  short_hash: string;
  message: string;
}

export interface BranchInfo {
  name: string;
  full_name: string;
  upstream: string | null;
  ahead: number;
  behind: number;
  current: boolean;
  detached: boolean;
}

export interface DiffLine {
  content: string;
  left_no: number | null;
  right_no: number | null;
  kind: "context" | "add" | "remove" | "header";
}

export interface DiffHunk {
  old_start: number;
  old_count: number;
  new_start: number;
  new_count: number;
  header: string;
  lines: DiffLine[];
}

export interface DiffFile {
  old_path: string | null;
  new_path: string | null;
  similarity: number | null;
  binary: boolean;
  hunks: DiffHunk[];
}

export interface DiffModel {
  source: "worktree" | "staged" | "commit" | "stash";
  old_revision: string | null;
  new_revision: string | null;
  files: DiffFile[];
}

export type RepoId = number;

/**
 * Unified invoke wrapper that converts Rust `AppError` (serialized with
 * `tag = "code"`, variant fields inlined) into a toast-friendly shape.
 */
export async function gitInvoke<T>(
  command: string,
  args?: Record<string, unknown>
): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (raw) {
    throw normalizeError(raw);
  }
}

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

// =====================
// Typed command API
// =====================

export const repo = {
  open: (path: string) =>
    gitInvoke<[RepoId, string]>("repo_open", { path }),
  close: (id: RepoId) => gitInvoke<void>("repo_close", { id }),
  list: () => gitInvoke<[RepoId, string][]>("repo_list"),
};

export const git = {
  status: (id: RepoId) => gitInvoke<FileStatus[]>("git_status", { id }),
  stage: (id: RepoId, paths: string[]) =>
    gitInvoke<void>("git_stage", { id, paths }),
  unstage: (id: RepoId, paths: string[]) =>
    gitInvoke<void>("git_unstage", { id, paths }),
  discard: (id: RepoId, paths: string[]) =>
    gitInvoke<void>("git_discard", { id, paths }),
  commit: (id: RepoId, message: string, amend = false, noVerify = false) =>
    gitInvoke<CommitResult>("git_commit", {
      id,
      message,
      amend,
      noVerify,
    }),
  log: (id: RepoId, limit = 200, offset = 0, paths?: string[]) =>
    gitInvoke<CommitInfo[]>("git_log", { id, limit, offset, paths }),
  diff: (
    id: RepoId,
    source: DiffModel["source"],
    oldRev?: string,
    newRev?: string,
    paths?: string[]
  ) =>
    gitInvoke<DiffModel>("git_diff", {
      id,
      source,
      oldRev,
      newRev,
      paths,
    }),
  branches: (id: RepoId) => gitInvoke<BranchInfo[]>("git_branches", { id }),
  checkoutBranch: (id: RepoId, name: string) =>
    gitInvoke<void>("git_checkout_branch", { id, name }),
};
