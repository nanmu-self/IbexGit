import { invoke } from "@tauri-apps/api/core";
import { addToast } from "$lib/stores/toast";
import type { AppError } from "$lib/stores/toast";

// =====================
// Types generated from Rust structs via ts-rs (cargo test regenerates
// src/lib/git/bindings/). Do not edit manually — edit the Rust structs.
// =====================

import type { BranchInfo } from "./bindings/BranchInfo";
import type { CommitInfo } from "./bindings/CommitInfo";
import type { CommitResult } from "./bindings/CommitResult";
import type { DiffFile } from "./bindings/DiffFile";
import type { DiffHunk } from "./bindings/DiffHunk";
import type { DiffLine } from "./bindings/DiffLine";
import type { DiffLineKind } from "./bindings/DiffLineKind";
import type { DiffModel } from "./bindings/DiffModel";
import type { DiffSource } from "./bindings/DiffSource";
import type { FileStatus } from "./bindings/FileStatus";

export type {
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
};

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
