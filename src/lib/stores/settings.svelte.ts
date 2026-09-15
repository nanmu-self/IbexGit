/**
 * Application settings (PLAN §4.4: `{appData}/settings.json` via
 * tauri-plugin-store). Also carries the open-repos session so a restart
 * restores the previous workspace (P2 acceptance).
 */
import { load, type Store } from "@tauri-apps/plugin-store";
import { setLocale, type Locale } from "$lib/i18n";

export type ThemeMode = "light" | "dark" | "system";

class SettingsStore {
  ready = $state(false);
  theme = $state<ThemeMode>("system");
  locale = $state<Locale>("zh-CN");
  showSidebar = $state(true);
  /** Sidebar width in px (global panel state). */
  sidebarWidth = $state(248);
  /** File-list column width inside the workspace view. */
  fileListWidth = $state(320);
  /** Session: repositories open at last quit + the active one. */
  openPaths = $state<string[]>([]);
  activePath = $state<string | null>(null);

  #store: Store | null = null;

  async init(): Promise<void> {
    if (this.#store) return;
    try {
      this.#store = await load("settings.json", { autoSave: true });
      this.theme = (await this.#store.get<ThemeMode>("theme")) ?? "system";
      this.locale = (await this.#store.get<Locale>("locale")) ?? "zh-CN";
      this.showSidebar = (await this.#store.get<boolean>("showSidebar")) ?? true;
      this.sidebarWidth =
        (await this.#store.get<number>("sidebarWidth")) ?? 248;
      this.fileListWidth =
        (await this.#store.get<number>("fileListWidth")) ?? 320;
      this.openPaths = (await this.#store.get<string[]>("openPaths")) ?? [];
      this.activePath = (await this.#store.get<string | null>("activePath")) ?? null;
    } finally {
      setLocale(this.locale);
      this.ready = true;
    }
  }

  async setTheme(theme: ThemeMode): Promise<void> {
    this.theme = theme;
    await this.#store?.set("theme", theme);
  }

  async setLocale(locale: Locale): Promise<void> {
    this.locale = locale;
    setLocale(locale);
    await this.#store?.set("locale", locale);
  }

  async setShowSidebar(show: boolean): Promise<void> {
    this.showSidebar = show;
    await this.#store?.set("showSidebar", show);
  }

  async setSidebarWidth(width: number): Promise<void> {
    this.sidebarWidth = width;
    await this.#store?.set("sidebarWidth", width);
  }

  async setFileListWidth(width: number): Promise<void> {
    this.fileListWidth = width;
    await this.#store?.set("fileListWidth", width);
  }

  async setSession(openPaths: string[], activePath: string | null): Promise<void> {
    this.openPaths = [...openPaths];
    this.activePath = activePath;
    await this.#store?.set("openPaths", this.openPaths);
    await this.#store?.set("activePath", activePath);
  }
}

export const settings = new SettingsStore();
