/**
 * Multi-repo tabs store (P2). Owns open repositories, the active tab,
 * recents, per-repo UI state persistence and the watcher→refresh pipeline.
 *
 * Svelte 5 runes: all mutable state is `$state`; the singleton class keeps
 * actions and derived getters together.
 */
import { git, repo, workspace, normalizeError } from "$lib/git";
import { onRepoChanged, onAppOpenPaths } from "$lib/git/events";
import type {
  BranchInfo,
  FileStatus,
  RecentRepo,
  RepoChanged,
  RepoGroup,
  RepoMeta,
  RepoUiState,
} from "$lib/git/bindings";
import { settings } from "./settings.svelte";
import { getPlatform } from "$lib/keyboard/keymap";

export type ViewId = "changes" | "history" | "tags";

/**
 * Sentinel id of the repositories overview tab (P3.5): opened by the "+"
 * button like a browser's new-tab page; lists all known repos with group
 * and bookmark management. It is not a repo and lives outside `tabs`.
 */
export const REPOS_TAB_ID = "__repos__";

export interface RepoTab {
  /** Backend RepoId (FNV-1a of path, string-serialized on the wire). */
  id: string;
  /** Resolved worktree root (backend may walk up from the requested path). */
  path: string;
  name: string;
  phase: "loading" | "ready" | "error";
  error: string | null;
  branch: string;
  detached: boolean;
  ahead: number;
  behind: number;
  files: FileStatus[];
  branches: BranchInfo[];
  refreshing: boolean;
  /** Last status+branches round trip (SLA 埋点: IPC + re-read portion). */
  lastRefreshMs: number | null;
}

/** Frontend-facing UI state (serialized via `RepoUiState`, PLAN §4.4). */
export interface UiState {
  view: ViewId;
  filter: string;
  sidebar_collapsed: string[];
  selected_file: { path: string; source: "worktree" | "staged" } | null;
  /** Workspace list presentation (P3): flat sections or path tree. */
  view_mode: "list" | "tree";
  /** Collapsed directory ids in tree mode. */
  tree_collapsed: string[];
}

function defaultUi(): UiState {
  return {
    view: "changes",
    filter: "",
    sidebar_collapsed: [],
    selected_file: null,
    view_mode: "list",
    tree_collapsed: [],
  };
}

function coerceUi(raw: Partial<RepoUiState> | null | undefined): UiState {
  const ui = defaultUi();
  if (!raw) return ui;
  if (raw.view === "changes" || raw.view === "history" || raw.view === "tags") {
    ui.view = raw.view;
  }
  if (typeof raw.filter === "string") ui.filter = raw.filter;
  if (Array.isArray(raw.sidebar_collapsed)) {
    ui.sidebar_collapsed = raw.sidebar_collapsed.filter((s) => typeof s === "string");
  }
  if (
    raw.selected_file &&
    typeof raw.selected_file.path === "string" &&
    (raw.selected_file.source === "worktree" || raw.selected_file.source === "staged")
  ) {
    ui.selected_file = { path: raw.selected_file.path, source: raw.selected_file.source };
  }
  if (raw.view_mode === "list" || raw.view_mode === "tree") ui.view_mode = raw.view_mode;
  if (Array.isArray(raw.tree_collapsed)) {
    ui.tree_collapsed = raw.tree_collapsed.filter((s) => typeof s === "string");
  }
  return ui;
}

function samePath(a: string, b: string): boolean {
  if (a === b) return true;
  return getPlatform() === "windows" ? a.toLowerCase() === b.toLowerCase() : false;
}
export { samePath };

/**
 * Split status items into the workspace sections (P3: conflicts get their
 * own section and are excluded from staged/unstaged; untracked = unstaged).
 */
export function splitFiles(files: FileStatus[]): {
  conflicts: FileStatus[];
  staged: FileStatus[];
  unstaged: FileStatus[];
} {
  return {
    conflicts: files.filter((f) => f.conflict),
    staged: files.filter((f) => f.staged && !f.conflict),
    unstaged: files.filter((f) => !f.conflict && (f.unstaged || f.untracked)),
  };
}

class ReposStore {
  tabs = $state<RepoTab[]>([]);
  activeId = $state<string | null>(null);
  recent = $state<RecentRepo[]>([]);
  /** UI state of the ACTIVE repository (swapped on tab switch). */
  ui = $state<UiState>(defaultUi());
  /** Groups sorted by display order (P3.5, from groups.json). */
  groups = $state<RepoGroup[]>([]);
  /** Resolved repo path → organizational metadata (P3.5). */
  repoMeta = $state<Record<string, RepoMeta>>({});
  /** Whether the repositories overview tab is open (P3.5). */
  reposTabOpen = $state(false);

  #saveTimer: ReturnType<typeof setTimeout> | null = null;

  get active(): RepoTab | null {
    if (this.activeId === REPOS_TAB_ID) return null;
    return this.tabs.find((t) => t.id === this.activeId) ?? null;
  }

  get reposTabActive(): boolean {
    return this.activeId === REPOS_TAB_ID;
  }

  get activeChanged(): number {
    return this.active?.files.length ?? 0;
  }

  // ===================== open / close / activate =====================

  async openPath(path: string): Promise<void> {
    const trimmed = path.trim();
    if (!trimmed) return;
    const existing = this.tabs.find((t) => samePath(t.path, trimmed));
    if (existing) {
      this.activate(existing.id);
      return;
    }
    try {
      const [id, resolved] = await repo.open(trimmed);
      if (this.tabs.some((t) => t.id === id || samePath(t.path, resolved))) {
        this.activate(id);
        return;
      }
      const name = resolved.split(/[\\/]/).filter(Boolean).pop() ?? resolved;
      this.tabs.push({
        id,
        path: resolved,
        name,
        phase: "loading",
        error: null,
        branch: "",
        detached: false,
        ahead: 0,
        behind: 0,
        files: [],
        branches: [],
        refreshing: false,
        lastRefreshMs: null,
      });
      this.activate(id);
      await this.#load(id);
      workspace.touchRecent(resolved, name)
        .then(() => this.loadRecents())
        .catch(() => {});
    } catch (raw) {
      // normalizeError already surfaced a toast.
      normalizeError(raw);
    }
  }

  async close(id: string, closeOthers = false): Promise<void> {
    if (id === REPOS_TAB_ID) {
      this.reposTabOpen = false;
      if (this.activeId === REPOS_TAB_ID) {
        this.activeId = this.tabs[0]?.id ?? null;
        if (this.activeId !== null) this.activate(this.activeId);
        else this.ui = defaultUi();
      }
      await this.#persistSession();
      return;
    }
    const targets = closeOthers
      ? this.tabs.filter((t) => t.id !== id).map((t) => t.id)
      : [id];
    const closingActive = this.activeId !== null && targets.includes(this.activeId);
    if (closingActive && this.active) {
      // Flush the UI state of the repo going away before switching.
      workspace.saveState(this.active.path, this.#toBinding(this.ui)).catch(() => {});
    }
    for (const target of targets) {
      const idx = this.tabs.findIndex((t) => t.id === target);
      if (idx === -1) continue;
      this.tabs.splice(idx, 1);
      repo.close(target).catch(() => {});
    }
    if (closingActive) {
      // Fall back to another repo tab, else the repos tab when open.
      const fallback = this.tabs[0]?.id ?? (this.reposTabOpen ? REPOS_TAB_ID : null);
      if (fallback !== null && fallback !== REPOS_TAB_ID) {
        this.activate(fallback);
      } else if (fallback === REPOS_TAB_ID) {
        this.activeId = REPOS_TAB_ID;
        this.ui = defaultUi();
      } else {
        this.activeId = null;
        this.ui = defaultUi();
      }
    }
    await this.#persistSession();
  }

  /** Open (or focus) the repositories overview tab ("+", P3.5). */
  openReposTab(): void {
    if (!this.reposTabOpen) {
      this.reposTabOpen = true;
      if (this.active) {
        workspace.saveState(this.active.path, this.#toBinding(this.ui)).catch(() => {});
      }
    }
    this.activeId = REPOS_TAB_ID;
    void this.#persistSession();
  }

  activate(id: string): void {
    if (id === REPOS_TAB_ID) {
      this.openReposTab();
      return;
    }
    const tab = this.tabs.find((t) => t.id === id);
    if (!tab) return;
    if (this.activeId === id) return;
    // Save the outgoing repo's UI state, then swap in the new one's.
    if (this.active) {
      workspace.saveState(this.active.path, this.#toBinding(this.ui)).catch(() => {});
    }
    this.activeId = id;
    this.ui = defaultUi();
    workspace
      .loadState(tab.path)
      .then((state) => {
        if (this.activeId === id) this.ui = coerceUi(state);
      })
      .catch(() => {});
    void this.#persistSession();
  }

  // ===================== data loading / refresh =====================

  async refresh(id: string): Promise<void> {
    const tab = this.tabs.find((t) => t.id === id);
    if (!tab || tab.refreshing) return;
    tab.refreshing = true;
    const t0 = performance.now();
    try {
      const [files, branches] = await Promise.all([
        git.status(id),
        git.branches(id),
      ]);
      this.#apply(id, files, branches, performance.now() - t0);
    } catch (raw) {
      const e = normalizeError(raw);
      const t = this.tabs.find((x) => x.id === id);
      if (t) {
        t.phase = "error";
        t.error = e.message;
      }
    } finally {
      const t = this.tabs.find((x) => x.id === id);
      if (t) t.refreshing = false;
    }
  }

  async #load(id: string): Promise<void> {
    await this.refresh(id);
  }

  #apply(id: string, files: FileStatus[], branches: BranchInfo[], ms: number): void {
    const tab = this.tabs.find((t) => t.id === id);
    if (!tab) return;
    tab.files = files;
    tab.branches = branches;
    const current = branches.find((b) => b.current);
    tab.branch = current?.name ?? "";
    tab.detached = current?.detached ?? false;
    tab.ahead = current?.ahead ?? 0;
    tab.behind = current?.behind ?? 0;
    tab.phase = "ready";
    tab.error = null;
    tab.lastRefreshMs = ms;
  }

  // ===================== per-repo UI state =====================

  updateUi(patch: Partial<UiState>): void {
    this.ui = { ...this.ui, ...patch };
    this.#scheduleUiSave();
  }

  #scheduleUiSave(): void {
    if (this.#saveTimer) clearTimeout(this.#saveTimer);
    this.#saveTimer = setTimeout(() => {
      this.#saveTimer = null;
      this.#saveUiNow();
    }, 600);
  }

  #saveUiNow(): void {
    const tab = this.active;
    if (!tab) return;
    workspace.saveState(tab.path, this.#toBinding(this.ui)).catch(() => {});
  }

  #toBinding(ui: UiState): RepoUiState {
    return {
      view: ui.view,
      filter: ui.filter,
      sidebar_collapsed: ui.sidebar_collapsed,
      selected_file: ui.selected_file
        ? { path: ui.selected_file.path, source: ui.selected_file.source }
        : null,
      view_mode: ui.view_mode,
      tree_collapsed: ui.tree_collapsed,
    };
  }

  // ===================== groups + stars (P3.5) =====================

  /** Normalized metadata of one repo (defaults when unrecorded). */
  metaFor(path: string): { group_id: string | null; bookmark: string | null } {
    let raw = this.repoMeta[path];
    if (!raw) {
      for (const [key, meta] of Object.entries(this.repoMeta)) {
        if (samePath(key, path)) {
          raw = meta;
          break;
        }
      }
    }
    return { group_id: raw?.group_id ?? null, bookmark: raw?.bookmark ?? null };
  }

  groupOf(path: string): string | null {
    return this.metaFor(path).group_id;
  }

  isBookmarked(path: string, id: string): boolean {
    return this.metaFor(path).bookmark === id;
  }

  bookmarkOf(path: string): string | null {
    return this.metaFor(path).bookmark;
  }

  groupName(id: string | null): string | null {
    if (!id) return null;
    return this.groups.find((g) => g.id === id)?.name ?? null;
  }

  async loadGroups(): Promise<void> {
    try {
      const file = await workspace.groups();
      this.groups = [...(file.groups ?? [])].sort((a, b) => a.order - b.order);
      this.repoMeta = { ...(file.repos ?? {}) };
    } catch {
      this.groups = [];
      this.repoMeta = {};
    }
  }

  /** Optimistic local write; meaningless entries are dropped (mirrors backend). */
  #setMetaLocal(path: string, meta: RepoMeta): void {
    const next = { ...this.repoMeta };
    if (meta.group_id || meta.bookmark) next[path] = meta;
    else delete next[path];
    this.repoMeta = next;
  }

  async createGroup(name: string): Promise<RepoGroup> {
    const g = await workspace.upsertGroup(null, name);
    this.groups = [...this.groups, g].sort((a, b) => a.order - b.order);
    return g;
  }

  async renameGroup(id: string, name: string): Promise<void> {
    await workspace.upsertGroup(id, name);
    this.groups = this.groups.map((g) => (g.id === id ? { ...g, name } : g));
  }

  async deleteGroup(id: string): Promise<void> {
    await workspace.deleteGroup(id);
    this.groups = this.groups.filter((g) => g.id !== id);
    // Members fall back to ungrouped; bookmarks are kept (backend semantics).
    const next: Record<string, RepoMeta> = {};
    for (const [path, meta] of Object.entries(this.repoMeta)) {
      if (meta.group_id === id) {
        if (meta.bookmark) next[path] = { group_id: null, bookmark: meta.bookmark };
      } else {
        next[path] = meta;
      }
    }
    this.repoMeta = next;
  }

  async setRepoGroup(path: string, groupId: string | null): Promise<void> {
    const meta = { ...this.metaFor(path), group_id: groupId };
    this.#setMetaLocal(path, meta);
    try {
      await workspace.updateRepo(path, meta);
    } catch (raw) {
      normalizeError(raw);
      await this.loadGroups(); // resync after failed persist
    }
  }

  async setBookmark(path: string, bookmarkId: string | null): Promise<void> {
    const meta = { ...this.metaFor(path), bookmark: bookmarkId };
    this.#setMetaLocal(path, meta);
    try {
      await workspace.updateRepo(path, meta);
    } catch (raw) {
      normalizeError(raw);
      await this.loadGroups();
    }
  }

  // ===================== recents + session =====================

  async loadRecents(): Promise<void> {
    try {
      this.recent = await workspace.recents();
    } catch {
      this.recent = [];
    }
  }

  async forgetRecent(path: string): Promise<void> {
    try {
      await workspace.forgetRecent(path);
    } finally {
      await this.loadRecents();
    }
  }

  async #persistSession(): Promise<void> {
    await settings.setSession(
      this.tabs.map((t) => t.path),
      this.reposTabActive ? null : (this.active?.path ?? null),
      this.reposTabOpen,
    );
  }

  /** Restore the last session (called once after settings are ready). */
  async restoreSession(): Promise<void> {
    await Promise.all([this.loadRecents(), this.loadGroups()]);
    for (const path of settings.openPaths) {
      await this.openPath(path);
    }
    if (settings.reposTabOpen) {
      this.openReposTab();
    }
    if (settings.activePath) {
      const tab = this.tabs.find((t) => samePath(t.path, settings.activePath ?? ""));
      if (tab) this.activate(tab.id);
    }
  }

  // ===================== backend events (watcher / single-instance) ====

  /** Wire backend events; returns an unlisten function. */
  initEvents(): () => void {
    const unChanged = onRepoChanged((payload: RepoChanged) => {
      void this.#onRepoChanged(payload);
    });
    const unOpen = onAppOpenPaths((payload) => {
      for (const path of payload.paths) void this.openPath(path);
    });
    return () => {
      void unChanged.then((off) => off());
      void unOpen.then((off) => off());
    };
  }

  async #onRepoChanged(payload: RepoChanged): Promise<void> {
    const tab = this.tabs.find((t) => t.id === payload.repoId);
    if (!tab) return;
    const t0 = performance.now();
    await this.refresh(tab.id);
    // SLA 埋点 (PLAN §4.3): IPC + re-read portion of the end-to-end budget;
    // fs-event→notify→debounce is instrumented on the Rust side.
    console.debug(
      `[sla] repo-refresh repo=${tab.name} gen=${payload.generation} ipc+reread=${(performance.now() - t0).toFixed(0)}ms`,
    );
  }
}

export const repos = new ReposStore();
