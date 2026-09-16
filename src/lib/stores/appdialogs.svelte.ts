/**
 * Global app-level dialog state (P10): command palette, settings center,
 * shortcuts / about dialogs. Opened from the feature registry (menu,
 * command palette, keyboard), rendered once in +layout.svelte.
 *
 * Svelte 5 runes: plain `$state` module object（同 netdialogs 模式）.
 */

export type SettingsSection =
  | "general"
  | "git"
  | "network"
  | "credentials"
  | "tools"
  | "gitconfig"
  | "advanced";

class AppDialogsStore {
  paletteOpen = $state(false);
  settingsOpen = $state(false);
  settingsSection = $state<SettingsSection>("general");
  shortcutsOpen = $state(false);
  aboutOpen = $state(false);

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
}

export const appDialogs = new AppDialogsStore();
