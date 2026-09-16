/**
 * Refs/branch action bus (P6). Decouples entry points (sidebar RefPanel,
 * toolbar buttons, history view, tags view) from the dialog host
 * (`RefsDialogsHost`) which owns every refs dialog and executor.
 */

export type RefAction =
  | { kind: "newBranch"; start?: string }
  | { kind: "renameBranch"; name: string }
  | { kind: "setUpstream"; branch: string; upstream?: string | null }
  | { kind: "deleteBranch"; name: string }
  | { kind: "reset"; initialTarget?: string }
  | { kind: "clean" }
  | { kind: "backups" }
  | { kind: "newTag"; target?: string }
  | { kind: "deleteTag"; name: string }
  | { kind: "addRemote" }
  | { kind: "editRemote"; name: string }
  | { kind: "removeRemote"; name: string }
  | { kind: "fetch"; remote?: string | null }
  | { kind: "pull" }
  | { kind: "push" }
  | { kind: "merge"; target: string }
  | { kind: "rebase"; target: string }
  | { kind: "compare"; left: string; right?: string }
  | { kind: "stashView"; index: number }
  | { kind: "stashDrop"; index: number }
  /** Reset the branch hard to a reflog/backup record (track B UI). */
  | { kind: "reflogRestore"; hash: string; subject: string };

const listeners = new Set<(a: RefAction) => void>();

/** Subscribe to ref actions; returns an unsubscribe function. */
export function onRefAction(fn: (a: RefAction) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Fire a ref action (handled by RefsDialogsHost). */
export function requestRefAction(a: RefAction): void {
  for (const l of [...listeners]) l(a);
}
