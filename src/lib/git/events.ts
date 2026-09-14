import { events, type RepoChanged } from "./bindings";

export type { RepoChanged } from "./bindings";
export type { UnlistenFn } from "@tauri-apps/api/event";

/**
 * Subscribe to repository change events (state invalidation system,
 * PLAN §4.3): emitted by the backend after a change was detected, debounced,
 * and the caches re-read.
 */
export function onRepoChanged(
  handler: (payload: RepoChanged) => void
): Promise<() => void> {
  return events.repoChanged.listen((e) => handler(e.payload));
}
