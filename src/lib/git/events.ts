import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Payload of the `repo://changed` event emitted by the backend after an
 * external (or internal) repository change was detected, debounced, and the
 * caches re-read (state invalidation system, PLAN §4.3).
 */
export interface RepoChangedPayload {
  /** Backend RepoId (FNV hash of the worktree path). */
  repoId: number;
  /** Which cache domains changed: head | index | refs | merge_state | worktree | config */
  kinds: string[];
  /** Monotonically increasing per-repo generation; use to drop stale reads. */
  generation: number;
}

/**
 * Subscribe to repository change events. Returns an unlisten function —
 * call it on component teardown.
 */
export function onRepoChanged(
  handler: (payload: RepoChangedPayload) => void
): Promise<UnlistenFn> {
  return listen<RepoChangedPayload>("repo://changed", (e) => handler(e.payload));
}
