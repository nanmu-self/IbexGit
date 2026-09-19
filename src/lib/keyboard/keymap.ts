/**
 * Keymap single source of truth (PLAN §4.9).
 *
 * - `Mod` is the platform primary modifier: Ctrl on Windows/Linux, Cmd on
 *   macOS. Bindings are declared once; per-platform exceptions go into the
 *   optional `mac` override (e.g. macOS-reserved ⌘Q/⌘W/⌘Tab are avoided).
 * - The native menu accelerators (P10) and the command palette must be
 *   generated from this table too — no second source.
 */

export type Platform = "macos" | "windows" | "linux";

export interface Binding {
  /** Platform primary modifier (Win/Linux = Ctrl, macOS = Cmd). */
  mod?: boolean;
  /** Literal Ctrl (mainly for macOS overrides that must avoid ⌘). */
  ctrl?: boolean;
  alt?: boolean;
  shift?: boolean;
  /** Canonical key name, lowercase: letters, digits, 'f5', 'enter', 'arrowright'… */
  key: string;
  /** Per-platform override (used on macOS to dodge reserved combos). */
  mac?: Omit<Binding, "mac">;
}

/**
 * P2 action set. Unbound features (e.g. app.quit) are intentionally absent:
 * ⌘Q is macOS-reserved and closing the window is the native gesture.
 */
export const keymap = {
  "repo.open": { mod: true, key: "o" },
  "repo.refresh": { key: "f5" },
  "repo.closeTab": { mod: true, key: "w", mac: { ctrl: true, key: "w" } },
  "repo.nextTab": { mod: true, alt: true, key: "arrowright" },
  "repo.prevTab": { mod: true, alt: true, key: "arrowleft" },
  "repo.fetch": { mod: true, alt: true, key: "f" },
  "repo.pull": { mod: true, alt: true, key: "p" },
  "repo.push": { mod: true, alt: true, key: "u" },
  "repo.stash": { mod: true, alt: true, key: "s" },
  "repo.newBranch": { mod: true, alt: true, key: "b" },
  "repo.newTag": { mod: true, alt: true, key: "t" },
  "view.toggleSidebar": { mod: true, key: "b" },
  "view.toggleTheme": {
    mod: true,
    shift: true,
    key: "t",
    mac: { ctrl: true, shift: true, key: "t" },
  },
  "view.changes": { mod: true, key: "1" },
  "view.history": { mod: true, key: "2" },
  "view.tags": { mod: true, key: "3" },
  "workspace.focusFilter": { mod: true, shift: true, key: "f" },
  "app.palette": { mod: true, shift: true, key: "p" },
  "app.settings": { mod: true, key: "," },
  "app.tasks": { mod: true, key: "j" },
} satisfies Record<string, Binding>;

export type ActionId = keyof typeof keymap;

export function getPlatform(): Platform {
  const ua = `${navigator.userAgent} ${navigator.platform ?? ""}`.toLowerCase();
  if (ua.includes("mac")) return "macos";
  if (ua.includes("win")) return "windows";
  return "linux";
}

interface Resolved {
  ctrl: boolean;
  meta: boolean;
  alt: boolean;
  shift: boolean;
  key: string;
}

/** Resolve a binding into absolute modifier state for the given platform. */
function resolve(binding: Binding, platform: Platform): Resolved {
  const src = platform === "macos" && binding.mac ? binding.mac : binding;
  return {
    meta: platform === "macos" && (src.mod ?? false),
    ctrl: platform === "macos" ? (src.ctrl ?? false) : (src.mod ?? false) || (src.ctrl ?? false),
    alt: src.alt ?? false,
    shift: src.shift ?? false,
    key: src.key,
  };
}

/** Strict match: every declared modifier must be present, none extra. */
export function matchesBinding(
  event: KeyboardEvent,
  binding: Binding,
  platform: Platform,
): boolean {
  const r = resolve(binding, platform);
  if (event.ctrlKey !== r.ctrl) return false;
  if (event.metaKey !== r.meta) return false;
  if (event.altKey !== r.alt) return false;
  if (event.shiftKey !== r.shift) return false;
  return event.key.toLowerCase() === r.key;
}

const KEY_LABELS: Record<string, string> = {
  enter: "Enter",
  tab: "Tab",
  escape: "Esc",
  f5: "F5",
  arrowright: "→",
  arrowleft: "←",
  arrowup: "↑",
  arrowdown: "↓",
};

function keyLabel(key: string): string {
  const mapped = KEY_LABELS[key];
  if (mapped) return mapped;
  return key.length === 1 ? key.toUpperCase() : key;
}

/** Platform-aware display: "Ctrl+Shift+T" on Win/Linux, "⌃⇧T" on macOS. */
export function formatBinding(binding: Binding, platform: Platform): string {
  const r = resolve(binding, platform);
  if (platform === "macos") {
    let out = "";
    if (r.ctrl) out += "⌃";
    if (r.alt) out += "⌥";
    if (r.shift) out += "⇧";
    if (r.meta) out += "⌘";
    return out + keyLabel(r.key);
  }
  const parts: string[] = [];
  if (r.ctrl) parts.push("Ctrl");
  if (r.alt) parts.push("Alt");
  if (r.shift) parts.push("Shift");
  parts.push(keyLabel(r.key));
  return parts.join("+");
}
