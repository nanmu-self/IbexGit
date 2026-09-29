/**
 * Lazy view registry — the single source for +page.svelte's dynamic imports.
 *
 * Loader constants MUST live here (module scope), not inline at call sites:
 * the shared cache below keys on loader function identity, so an inline
 * arrow (recreated on every parent render / HMR update) would silently
 * defeat the cache and reintroduce a spinner flash on every view switch.
 */
import type { Component } from "svelte";

type ViewLoader = () => Promise<{ default: Component }>;

/** Loaded components by loader identity (module-level, survives remounts). */
const cache = new Map<ViewLoader, Component>();

/** In-flight loads by loader identity: dedupes concurrent first loads
 *  (e.g. warmup effect racing the first LazyView mount). Cleared on
 *  failure so the next call retries. */
const pending = new Map<ViewLoader, Promise<Component>>();

/** Load the component behind a loader — cached, in-flight deduped. */
export function loadView(loader: ViewLoader): Promise<Component> {
  const hit = cache.get(loader);
  if (hit) return Promise.resolve(hit);
  const inflight = pending.get(loader);
  if (inflight) return inflight;
  const p = loader()
    .then((m) => {
      const comp = m.default;
      cache.set(loader, comp);
      return comp;
    })
    .finally(() => {
      pending.delete(loader);
    });
  pending.set(loader, p);
  return p;
}

/** Cache-only lookup (no load): lets LazyView render hit views synchronously. */
export function peekView(loader: ViewLoader): Component | undefined {
  return cache.get(loader);
}

/** Fire-and-forget warmup — e.g. the default view right after settings.ready. */
export function warmupView(loader: ViewLoader): void {
  void loadView(loader).catch(() => {});
}

// ---- loaders (stable module-level identity) ----

// Main-area views: mutually exclusive {#if} branches in +page.svelte.
export const workspaceLoader = () =>
  import("$lib/components/workspace/WorkspaceView.svelte");
export const historyLoader = () =>
  import("$lib/components/history/HistoryView.svelte");
export const tagsLoader = () => import("$lib/components/refs/TagsView.svelte");
export const reposTabLoader = () =>
  import("$lib/components/layout/ReposTab.svelte");

// Store-driven dialogs: defer until first open (sticky once mounted).
// RefsDialogsHost / NetDialogsHost must stay eagerly imported — they wire
// global event buses at mount time (refbus actions / net events).
export const fileInspectLoader = () =>
  import("$lib/components/file/FileInspectDialog.svelte");
export const gitErrorLoader = () =>
  import("$lib/components/giterror/GitErrorDialog.svelte");
export const reportLoader = () =>
  import("$lib/components/ai/ReportDialog.svelte");
