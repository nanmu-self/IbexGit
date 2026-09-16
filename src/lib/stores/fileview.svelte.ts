/**
 * File-inspect dialog state (P9 文件追溯): single-file history
 * (`log --follow`) + blame, hosted in one modal window opened from the
 * workspace file context menu, the commit-detail file list and conflict
 * entries. Transient by design — not part of persisted per-repo UI state.
 */

export type FileViewTab = "history" | "blame";

class FileViewStore {
  open = $state(false);
  /** Repo-relative path under inspection. */
  path = $state<string | null>(null);
  tab = $state<FileViewTab>("history");
  /** Commit to select when the history tab (re)opens — blame → diff jump. */
  jumpHash = $state<string | null>(null);

  show(path: string, tab: FileViewTab, jumpHash?: string): void {
    this.path = path;
    this.tab = tab;
    this.jumpHash = jumpHash ?? null;
    this.open = true;
  }

  /** Blame → click a line: jump to that commit's file diff (history tab). */
  jumpToCommit(hash: string): void {
    this.tab = "history";
    this.jumpHash = hash;
  }

  close(): void {
    this.open = false;
  }
}

export const fileView = new FileViewStore();
