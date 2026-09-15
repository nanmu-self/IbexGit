/**
 * Bookmark palette (P3.5): colorful bookmarks mark repositories for quick
 * visual identification — multiple colors, one bookmark per repo. The id is
 * persisted in `groups.json` (`RepoMeta.bookmark`); the color is a display
 * concern owned here. Add entries freely — unknown ids in stored data
 * render as unmarked.
 */
export interface BookmarkColor {
  id: string;
  /** Tailwind-independent hex so it survives dynamic usage (no purge issues). */
  color: string;
}

export const BOOKMARKS: BookmarkColor[] = [
  { id: "red", color: "#ef4444" },
  { id: "orange", color: "#f97316" },
  { id: "amber", color: "#f59e0b" },
  { id: "green", color: "#22c55e" },
  { id: "teal", color: "#14b8a6" },
  { id: "blue", color: "#3b82f6" },
  { id: "purple", color: "#a855f7" },
  { id: "pink", color: "#ec4899" },
];

export function bookmarkColor(id: string | null): BookmarkColor | null {
  if (!id) return null;
  return BOOKMARKS.find((b) => b.id === id) ?? null;
}
