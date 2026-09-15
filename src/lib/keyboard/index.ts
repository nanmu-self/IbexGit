import {
  keymap,
  matchesBinding,
  getPlatform,
  formatBinding,
  type ActionId,
  type Binding,
  type Platform,
} from "./keymap";

export type { ActionId, Binding, Platform };
export { keymap, matchesBinding, getPlatform, formatBinding };

type Handler = () => void;

const handlers = new Map<ActionId, Set<Handler>>();

/** Register a handler for an action; returns an unregister function. */
export function onAction(action: ActionId, handler: Handler): () => void {
  let set = handlers.get(action);
  if (!set) {
    set = new Set();
    handlers.set(action, set);
  }
  set.add(handler);
  return () => {
    set.delete(handler);
  };
}

/** Fire an action; returns false when nobody listens (keys stay untouched). */
export function emitAction(action: ActionId): boolean {
  const set = handlers.get(action);
  if (!set || set.size === 0) return false;
  for (const handler of [...set]) handler();
  return true;
}

/**
 * Global keydown dispatcher (capture phase). Only prevents default when an
 * action was actually handled, so unbound keys behave natively.
 */
export function initKeyboard(): () => void {
  const currentPlatform = getPlatform();
  const onKeyDown = (event: KeyboardEvent) => {
    for (const [action, binding] of Object.entries(keymap)) {
      if (!binding) continue;
      if (matchesBinding(event, binding, currentPlatform)) {
        if (emitAction(action as ActionId)) {
          event.preventDefault();
          event.stopPropagation();
          return;
        }
      }
    }
  };
  window.addEventListener("keydown", onKeyDown, true);
  return () => window.removeEventListener("keydown", onKeyDown, true);
}
