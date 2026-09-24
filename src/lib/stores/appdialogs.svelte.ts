/**
 * Global app-level dialog state (P10): command palette, settings center,
 * shortcuts / about dialogs. Opened from the feature registry (menu,
 * command palette, keyboard), rendered once in +layout.svelte.
 *
 * Svelte 5 runes: plain `$state` module object（同 netdialogs 模式）.
 */

import type { RepoId } from "$lib/git/bindings";

export type SettingsSection =
  | "general"
  | "git"
  | "network"
  | "credentials"
  | "ai"
  | "tools"
  | "gitconfig"
  | "advanced";

class AppDialogsStore {
  paletteOpen = $state(false);
  settingsOpen = $state(false);
  settingsSection = $state<SettingsSection>("general");
  shortcutsOpen = $state(false);
  aboutOpen = $state(false);
  /** 自动更新对话框（docs/auto-update-plan.md），由 updater 状态机/Toolbar 角标打开。 */
  updateOpen = $state(false);
  /** P11 日报/周报对话框。 */
  aiReportOpen = $state(false);
  /** 仓库级常用配置（.git/config）对话框；null = 当前活动仓库。 */
  repoSettingsOpen = $state(false);
  repoSettingsRepoId = $state<RepoId | null>(null);

  openPalette(): void {
    this.paletteOpen = true;
  }

  openSettings(section: SettingsSection = "general"): void {
    this.settingsSection = section;
    this.settingsOpen = true;
  }

  openShortcuts(): void {
    this.shortcutsOpen = true;
  }

  openAbout(): void {
    this.aboutOpen = true;
  }

  openUpdate(): void {
    this.updateOpen = true;
  }

  openAiReport(): void {
    this.aiReportOpen = true;
  }

  /** 仓库设置；`repoId` 为空时取当前活动仓库（菜单/命令面板入口）。 */
  openRepoSettings(repoId: RepoId | null = null): void {
    this.repoSettingsRepoId = repoId;
    this.repoSettingsOpen = true;
  }
}

export const appDialogs = new AppDialogsStore();
