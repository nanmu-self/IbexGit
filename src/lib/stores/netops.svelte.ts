/**
 * Network / long-op busy states per repository (P6: 状态栏与工具栏的
 * Fetch/Pull/Push 快捷动作进度提示). v1 uses a busy flag + completion
 * toast; the Task-center progress channel lands with the TaskManager UI.
 *
 * Svelte 5 runes: plain `$state` module object.
 */
import { git, normalizeError } from "$lib/git";
import { showToast } from "$lib/stores/toast";
import { giterr } from "$lib/stores/giterr.svelte";
import { t } from "$lib/i18n";
import { repos } from "$lib/stores/repos.svelte";
import type { RepoId } from "$lib/git/bindings";

export type NetOpKind = "fetch" | "pull" | "push";

export const netops = $state<{
  /** repoId → op kind currently running. */
  busy: Record<string, NetOpKind>;
}>({ busy: {} });

function setBusy(id: RepoId, kind: NetOpKind): void {
  netops.busy = { ...netops.busy, [id]: kind };
}

function clearBusy(id: RepoId): void {
  const next = { ...netops.busy };
  delete next[id];
  netops.busy = next;
}

export function isBusy(id: RepoId | null): boolean {
  return id !== null && netops.busy[id] !== undefined;
}

/**
 * P8: after a failed merge/pull/rebase/cherry-pick, detect whether a
 * conflict flow took over (MERGE_HEAD / rebase state set) → info toast
 * instead of an error; the OperationBanner guides from here.
 */
export async function conflictEntered(id: RepoId): Promise<boolean> {
  const op = await git.operationState(id).catch(() => null);
  if (op) {
    showToast("info", t("conflict.entered"));
    return true;
  }
  return false;
}

function runOp(
  id: RepoId,
  kind: NetOpKind,
  op: () => Promise<unknown>,
  onError?: (err: ReturnType<typeof normalizeError>) => void
): Promise<void> {
  if (netops.busy[id]) return Promise.resolve();
  setBusy(id, kind);
  return (async () => {
    try {
      await op();
    } catch (e) {
      const err = normalizeError(e);
      onError?.(err);
      // P7 取消语义：凭据框取消 / 用户取消 → “已取消”（非错误样式）。
      if (err.code === "credential_cancelled" || err.code === "operation_cancelled") {
        showToast("info", t("netops.cancelled"));
      }
    } finally {
      clearBusy(id);
    }
  })();
}

/** Fetch (all remotes when `remote` is null). */
export function runFetch(id: RepoId, remote?: string | null): Promise<void> {
  return runOp(id, "fetch", async () => {
    await git.fetch(id, remote ?? null);
    await repos.refresh(id);
    showToast("success", t("netops.fetchDone"));
  });
}

/** Pull with strategy; surfaces the outcome via toast. */
export function runPull(
  id: RepoId,
  remote: string | null,
  branch: string | null,
  mode: "merge" | "rebase" | "ff_only" | null
): Promise<void> {
  return runOp(
    id,
    "pull",
    async () => {
      const res = await git.pull(id, remote, branch, mode);
      await repos.refresh(id);
      if (res.success) {
        showToast("success", t("netops.pullDone"));
      } else if (!(await conflictEntered(id))) {
        showToast("error", res.message || t("netops.pullFailed"));
      }
    },
    (err) => {
      // 脏工作区拒绝 → 对话框提供“暂存并重试”（重跑同参数的 pull）。
      if (err.code === "dirty_worktree") {
        giterr.retry = () => {
          giterr.close();
          return runPull(id, remote, branch, mode);
        };
      }
    }
  );
}

/** Push; surfaces the outcome via toast. */
export function runPush(
  id: RepoId,
  remote: string,
  branch: string,
  opts: { forceWithLease?: boolean; setUpstream?: boolean; tags?: boolean } = {}
): Promise<void> {
  return runOp(id, "push", async () => {
    await git.push(
      id,
      remote,
      branch,
      opts.forceWithLease ?? false,
      opts.setUpstream ?? false,
      opts.tags ?? false
    );
    await repos.refresh(id);
    showToast("success", t("netops.pushDone"));
  });
}
