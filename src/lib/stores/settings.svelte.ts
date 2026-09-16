/**
 * Application settings (PLAN §4.4: `{appData}/settings.json` via
 * tauri-plugin-store). Also carries the open-repos session so a restart
 * restores the previous workspace (P2 acceptance).
 */
import { load, type Store } from "@tauri-apps/plugin-store";
import { commands } from "$lib/git/bindings";
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
  /** Detail panel width inside the history view (P5). */
  historyDetailWidth = $state(420);
  /** Session: repositories open at last quit + the active one. */
  openPaths = $state<string[]>([]);
  activePath = $state<string | null>(null);
  /** Whether the repositories overview tab ("+" tab, P3.5) was open. */
  reposTabOpen = $state(false);
  // ---- Diff viewer (P4) ----
  diffViewMode = $state<"unified" | "split">("unified");
  diffShowWhitespace = $state(false);
  diffSyntax = $state(true);
  // ---- 网络 / SSH (P7) ----
  sshKeyPath = $state("");
  proxyMode = $state<"inherit" | "none" | "custom">("inherit");
  proxyUrl = $state("");

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
      this.historyDetailWidth =
        (await this.#store.get<number>("historyDetailWidth")) ?? 420;
      this.openPaths = (await this.#store.get<string[]>("openPaths")) ?? [];
      this.activePath = (await this.#store.get<string | null>("activePath")) ?? null;
      this.reposTabOpen = (await this.#store.get<boolean>("reposTabOpen")) ?? false;
      this.diffViewMode =
        (await this.#store.get<"unified" | "split">("diffViewMode")) ?? "unified";
      this.diffShowWhitespace =
        (await this.#store.get<boolean>("diffShowWhitespace")) ?? false;
      this.diffSyntax = (await this.#store.get<boolean>("diffSyntax")) ?? true;
      this.sshKeyPath = (await this.#store.get<string>("sshKeyPath")) ?? "";
      this.proxyMode =
        (await this.#store.get<"inherit" | "none" | "custom">("proxyMode")) ?? "inherit";
      this.proxyUrl = (await this.#store.get<string>("proxyUrl")) ?? "";
    } finally {
      setLocale(this.locale);
      this.ready = true;
      // P7：把持久化的网络配置下发给 runner（失败不阻断启动，默认继承环境）。
      commands
        .appSetNetConfig({
          proxy_mode: this.proxyMode,
          proxy_url: this.proxyUrl || null,
          ssh_key_path: this.sshKeyPath || null,
        })
        .catch(() => {});
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

  async setHistoryDetailWidth(width: number): Promise<void> {
    this.historyDetailWidth = width;
    await this.#store?.set("historyDetailWidth", width);
  }

  async setDiffViewMode(mode: "unified" | "split"): Promise<void> {
    this.diffViewMode = mode;
    await this.#store?.set("diffViewMode", mode);
  }

  async setDiffShowWhitespace(v: boolean): Promise<void> {
    this.diffShowWhitespace = v;
    await this.#store?.set("diffShowWhitespace", v);
  }

  async setDiffSyntax(v: boolean): Promise<void> {
    this.diffSyntax = v;
    await this.#store?.set("diffSyntax", v);
  }

  /**
   * 保存网络配置并下发给 runner（P7：凭据注入在启动时已就位，
   * 代理与 SSH key 路径运行时更新）。
   */
  async setNetwork(cfg: {
    sshKeyPath: string;
    proxyMode: "inherit" | "none" | "custom";
    proxyUrl: string;
  }): Promise<void> {
    this.sshKeyPath = cfg.sshKeyPath;
    this.proxyMode = cfg.proxyMode;
    this.proxyUrl = cfg.proxyUrl;
    await this.#store?.set("sshKeyPath", cfg.sshKeyPath);
    await this.#store?.set("proxyMode", cfg.proxyMode);
    await this.#store?.set("proxyUrl", cfg.proxyUrl);
    await commands.appSetNetConfig({
      proxy_mode: cfg.proxyMode,
      proxy_url: cfg.proxyUrl || null,
      ssh_key_path: cfg.sshKeyPath || null,
    });
  }

  async setSession(
    openPaths: string[],
    activePath: string | null,
    reposTabOpen = false,
  ): Promise<void> {
    this.openPaths = [...openPaths];
    this.activePath = activePath;
    this.reposTabOpen = reposTabOpen;
    await this.#store?.set("openPaths", this.openPaths);
    await this.#store?.set("activePath", activePath);
    await this.#store?.set("reposTabOpen", reposTabOpen);
  }
}

export const settings = new SettingsStore();
