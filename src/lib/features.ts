/**
 * Feature registry (P10, PLAN §4.6 Feature 层)：可从菜单、命令面板、
 * 快捷键触达的每一个产品功能都在此登记 —— 标签（i18n）、所属菜单区、
 * 键位（引用 keymap 单一数据源，杜绝双源漂移）、执行动作。
 *
 * - `needsRepo` 的功能在无活动仓库时禁用（命令面板中隐藏）；
 * - `group` 用于菜单内的分隔符分组（同组内按 order 排序）；
 * - 命令面板按 id 记录最近使用（settings.recentFeatures）。
 */
import { repos } from "$lib/stores/repos.svelte";
import { settings } from "$lib/stores/settings.svelte";
import { requestRefAction } from "$lib/stores/refbus";
import { netDialogs } from "$lib/stores/netdialogs.svelte";
import { appDialogs } from "$lib/stores/appdialogs.svelte";
import { runFetch } from "$lib/stores/netops.svelte";
import { git, normalizeError } from "$lib/git";
import { showToast } from "$lib/stores/toast";
import { t } from "$lib/i18n";
import { emitAction, onAction, type ActionId } from "$lib/keyboard";
import { pickRepo } from "$lib/repo-picker";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

export type FeatureSection = "file" | "view" | "repository" | "help";

export interface Feature {
  id: string;
  labelKey: string;
  section: FeatureSection;
  /** Separator group inside the section (menu + palette grouping). */
  group: number;
  order: number;
  needsRepo: boolean;
  /** keymap action that also triggers this feature (single source of truth). */
  shortcut?: ActionId;
  /** Extra search terms (non-translated, matched case-insensitively). */
  keywords?: string;
  run: () => void;
}

function needsActiveRepo(): boolean {
  return repos.active !== null;
}

function refAction(a: Parameters<typeof requestRefAction>[0]): () => void {
  return () => {
    if (needsActiveRepo()) requestRefAction(a);
  };
}

function stashAll(): void {
  const id = repos.activeId;
  if (id === null) return;
  git
    .stashPush(id, null)
    .then(() => {
      showToast("success", t("refs.stash.pushed"));
      return repos.refresh(id);
    })
    .catch((e) => normalizeError(e));
}

function revealRepo(): void {
  const path = repos.active?.path;
  if (!path) return;
  revealItemInDir(path).catch((e) => normalizeError(e));
}

export const FEATURES: Feature[] = [
  // ---- File ----
  {
    id: "repo.open",
    labelKey: "menu.file.openRepo",
    section: "file",
    group: 0,
    order: 0,
    needsRepo: false,
    shortcut: "repo.open",
    keywords: "open repository folder",
    run: () => void pickRepo(),
  },
  {
    id: "repo.clone",
    labelKey: "menu.file.clone",
    section: "file",
    group: 0,
    order: 1,
    needsRepo: false,
    keywords: "clone url",
    run: () => netDialogs.openClone(),
  },
  {
    id: "repo.new",
    labelKey: "menu.file.newRepo",
    section: "file",
    group: 0,
    order: 2,
    needsRepo: false,
    keywords: "init empty repository",
    run: () => netDialogs.openNewRepo(),
  },
  {
    id: "repo.reveal",
    labelKey: "menu.file.reveal",
    section: "file",
    group: 1,
    order: 0,
    needsRepo: true,
    shortcut: "repo.reveal",
    keywords: "explorer finder show",
    run: revealRepo,
  },
  {
    id: "app.settings",
    labelKey: "settings.title",
    section: "file",
    group: 2,
    order: 0,
    needsRepo: false,
    shortcut: "app.settings",
    keywords: "preferences options settings",
    run: () => appDialogs.openSettings(),
  },

  // ---- View ----
  {
    id: "view.changes",
    labelKey: "sidebar.changes",
    section: "view",
    group: 0,
    order: 0,
    needsRepo: true,
    shortcut: "view.changes",
    keywords: "workspace status",
    run: () => {
      if (needsActiveRepo()) repos.updateUi({ view: "changes" });
    },
  },
  {
    id: "view.history",
    labelKey: "sidebar.history",
    section: "view",
    group: 0,
    order: 1,
    needsRepo: true,
    shortcut: "view.history",
    keywords: "commit graph log",
    run: () => {
      if (needsActiveRepo()) repos.updateUi({ view: "history" });
    },
  },
  {
    id: "view.tags",
    labelKey: "sidebar.tagsView",
    section: "view",
    group: 0,
    order: 2,
    needsRepo: true,
    shortcut: "view.tags",
    keywords: "tags view",
    run: () => {
      if (needsActiveRepo()) repos.updateUi({ view: "tags" });
    },
  },
  {
    id: "view.toggleSidebar",
    labelKey: "menu.view.toggleSidebar",
    section: "view",
    group: 1,
    order: 0,
    needsRepo: false,
    shortcut: "view.toggleSidebar",
    keywords: "sidebar panel",
    run: () => void settings.setShowSidebar(!settings.showSidebar),
  },
  {
    id: "view.toggleTheme",
    labelKey: "menu.view.toggleTheme",
    section: "view",
    group: 1,
    order: 1,
    needsRepo: false,
    shortcut: "view.toggleTheme",
    keywords: "dark light theme appearance",
    run: () => {
      const order = ["light", "dark", "system"] as const;
      const next = order[(order.indexOf(settings.theme) + 1) % order.length];
      void settings.setTheme(next);
    },
  },
  {
    id: "repo.refresh",
    labelKey: "menu.view.refresh",
    section: "view",
    group: 2,
    order: 0,
    needsRepo: true,
    shortcut: "repo.refresh",
    keywords: "reload status",
    run: () => {
      const id = repos.activeId;
      if (id !== null) void repos.refresh(id);
    },
  },

  // ---- Repository ----
  {
    id: "repo.fetch",
    labelKey: "menu.repo.fetch",
    section: "repository",
    group: 0,
    order: 0,
    needsRepo: true,
    shortcut: "repo.fetch",
    keywords: "download remote",
    run: () => {
      const id = repos.activeId;
      if (id !== null) void runFetch(id, null);
    },
  },
  {
    id: "repo.pull",
    labelKey: "menu.repo.pull",
    section: "repository",
    group: 0,
    order: 1,
    needsRepo: true,
    shortcut: "repo.pull",
    keywords: "update branch",
    run: refAction({ kind: "pull" }),
  },
  {
    id: "repo.push",
    labelKey: "menu.repo.push",
    section: "repository",
    group: 0,
    order: 2,
    needsRepo: true,
    shortcut: "repo.push",
    keywords: "upload commit remote",
    run: refAction({ kind: "push" }),
  },
  {
    id: "repo.stash",
    labelKey: "menu.repo.stash",
    section: "repository",
    group: 1,
    order: 0,
    needsRepo: true,
    shortcut: "repo.stash",
    keywords: "stash all changes",
    run: stashAll,
  },
  {
    id: "repo.newBranch",
    labelKey: "menu.repo.newBranch",
    section: "repository",
    group: 2,
    order: 0,
    needsRepo: true,
    shortcut: "repo.newBranch",
    keywords: "branch create",
    run: refAction({ kind: "newBranch" }),
  },
  {
    id: "repo.newTag",
    labelKey: "menu.repo.newTag",
    section: "repository",
    group: 2,
    order: 1,
    needsRepo: true,
    shortcut: "repo.newTag",
    keywords: "tag create",
    run: refAction({ kind: "newTag" }),
  },
  {
    id: "repo.compare",
    labelKey: "menu.repo.compare",
    section: "repository",
    group: 3,
    order: 0,
    needsRepo: true,
    keywords: "diff ahead behind branches",
    run: () => {
      const branch = repos.active?.branch;
      if (branch) requestRefAction({ kind: "compare", left: branch });
    },
  },
  {
    id: "repo.reset",
    labelKey: "menu.repo.reset",
    section: "repository",
    group: 3,
    order: 1,
    needsRepo: true,
    keywords: "soft mixed hard recover",
    run: refAction({ kind: "reset" }),
  },
  {
    id: "repo.clean",
    labelKey: "menu.repo.clean",
    section: "repository",
    group: 3,
    order: 2,
    needsRepo: true,
    keywords: "untracked delete remove",
    run: refAction({ kind: "clean" }),
  },
  {
    id: "repo.backups",
    labelKey: "menu.repo.backups",
    section: "repository",
    group: 3,
    order: 3,
    needsRepo: true,
    keywords: "orphan backup refs recovery gc",
    run: refAction({ kind: "backups" }),
  },
  {
    id: "repo.addRemote",
    labelKey: "menu.repo.remotes",
    section: "repository",
    group: 4,
    order: 0,
    needsRepo: true,
    keywords: "remote add manage url",
    run: refAction({ kind: "addRemote" }),
  },

  // ---- Help ----
  {
    id: "app.palette",
    labelKey: "palette.title",
    section: "help",
    group: 0,
    order: 0,
    needsRepo: false,
    shortcut: "app.palette",
    keywords: "command palette search",
    run: () => appDialogs.openPalette(),
  },
  {
    id: "app.shortcuts",
    labelKey: "menu.help.shortcuts",
    section: "help",
    group: 0,
    order: 1,
    needsRepo: false,
    keywords: "keymap keybindings",
    run: () => appDialogs.openShortcuts(),
  },
  {
    id: "app.about",
    labelKey: "menu.help.about",
    section: "help",
    group: 0,
    order: 2,
    needsRepo: false,
    keywords: "version info",
    run: () => appDialogs.openAbout(),
  },
];

/** Features of one section, sorted for menu rendering (groups → order). */
export function featuresForSection(section: FeatureSection): Feature[] {
  return FEATURES.filter((f) => f.section === section).sort(
    (a, b) => a.group - b.group || a.order - b.order,
  );
}

/** Group numbers present in a section, ascending (for separators). */
export function sectionGroups(section: FeatureSection): number[] {
  return [...new Set(featuresForSection(section).map((f) => f.group))];
}

/**
 * Wire every feature-declared shortcut through the action bus (keymap stays
 * the single source; features provide the handlers). Returns an unlisten fn.
 */
export function initFeatureShortcuts(): () => void {
  const offs = FEATURES.filter((f) => f.shortcut).map((f) =>
    onAction(f.shortcut as ActionId, () => f.run()),
  );
  return () => offs.forEach((off) => off());
}

/** Fire a feature through the action bus so keymap handlers stay in sync. */
export function runFeatureById(id: string): void {
  const f = FEATURES.find((x) => x.id === id);
  if (f) f.run();
  else emitAction(id as ActionId);
}
