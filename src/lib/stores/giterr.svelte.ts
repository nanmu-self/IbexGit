/**
 * Git failure dialog state (P12 友好错误): structured, high-frequency git
 * refusals (dirty worktree family) surface here instead of a raw-stderr
 * toast — same idea as VS Code's "clean your repository working tree"
 * dialog, plus a file list and a stash action.
 *
 * Fed from `normalizeError` (lib/git) for dialog-worthy error codes; the
 * network-op runners may attach a `retry` closure so the dialog can offer
 * "stash and retry". Svelte 5 runes: plain `$state` module object.
 */

export interface GitErrorInfo {
  /** AppError code, e.g. "dirty_worktree". */
  code: string;
  /** Humanized one-liner (toast fallback / dialog title). */
  message: string;
  /** Raw stderr, shown by the "command output" toggle. */
  stderr: string;
  /** Operation git refused, as git names it (merge/checkout/rebase/…). */
  operation: string | null;
  /** Conflicting paths git listed; empty when git names none. */
  files: string[];
  /** Blocked paths are untracked files (plain stash won't clear them). */
  untracked: boolean;
}

class GitErrorStore {
  open = $state(false);
  info = $state<GitErrorInfo | null>(null);
  showRaw = $state(false);
  busy = $state(false);
  /** Optional re-run hook attached by the op runner (e.g. runPull). */
  retry = $state<(() => Promise<void>) | null>(null);

  openFromError(
    e: { code: string; message: string; detail: string | null },
    raw: Record<string, unknown> = {}
  ): void {
    this.info = {
      code: e.code,
      message: e.message,
      stderr: typeof raw.stderr === "string" ? raw.stderr : (e.detail ?? ""),
      operation:
        typeof raw.operation === "string" && raw.operation.length > 0
          ? raw.operation
          : null,
      files: Array.isArray(raw.files)
        ? raw.files.filter((f): f is string => typeof f === "string")
        : [],
      untracked: raw.untracked === true,
    };
    this.showRaw = false;
    this.retry = null;
    this.open = true;
  }

  close(): void {
    this.open = false;
    this.info = null;
    this.retry = null;
    this.busy = false;
  }
}

export const giterr = new GitErrorStore();
