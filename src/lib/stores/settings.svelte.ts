/**
 * Application settings (PLAN §4.4: `{appData}/settings.json` via
 * tauri-plugin-store). Also carries the open-repos session so a restart
 * restores the previous workspace (P2 acceptance).
 */
import { load, type Store } from "@tauri-apps/plugin-store";
import { commands } from "$lib/git/bindings";
import { setLocale, t, type Locale } from "$lib/i18n";
import { showToast } from "$lib/stores/toast";

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
  /** Commit-list column width inside the file-history dialog (P9). */
  fileHistoryListWidth = $state(340);
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
  // ---- P10 设置中心 / 命令面板 ----
  /** Editor font family (diff / conflict editor); empty = default mono. */
  editorFont = $state("");
  /** Tab width for the conflict editor (CSS tab-size). */
  editorTabSize = $state(4);
  /** Default pull strategy (PullDialog default selection). */
  pullStrategy = $state<"merge" | "rebase" | "ff_only">("merge");
  /** Default push upstream behaviour (PushDialog checkbox default). */
  pushSetUpstream = $state<"whenMissing" | "always" | "never">("whenMissing");
  /** Whether the PushDialog starts with "include tags" pre-checked. */
  pushIncludeTags = $state(false);
  /** Default external merge tool ("" = git default; "custom" = mergeToolCmd). */
  mergeToolId = $state("");
  /** Custom mergetool command template (uses $REMOTE $LOCAL $BASE $MERGED). */
  mergeToolCmd = $state("");
  /** Tracing filter (P10 高级）；empty = built-in default. */
  logLevel = $state("");
  /** Command palette: recently executed feature ids (MRU, max 10). */
  recentFeatures = $state<string[]>([]);
  /** Custom git executable (P10）；empty = PATH "git". Requires restart. */
  gitPath = $state("");
  /** 设置中心弹窗尺寸（自由拖拽握把）；0 = 使用默认尺寸。 */
  settingsWidth = $state(0);
  settingsHeight = $state(0);

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
      this.fileHistoryListWidth =
        (await this.#store.get<number>("fileHistoryListWidth")) ?? 340;
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
      this.editorFont = (await this.#store.get<string>("editorFont")) ?? "";
      this.editorTabSize = (await this.#store.get<number>("editorTabSize")) ?? 4;
      this.pullStrategy =
        (await this.#store.get<"merge" | "rebase" | "ff_only">("pullStrategy")) ?? "merge";
      this.pushSetUpstream =
        (await this.#store.get<"whenMissing" | "always" | "never">("pushSetUpstream")) ??
        "whenMissing";
      this.pushIncludeTags = (await this.#store.get<boolean>("pushIncludeTags")) ?? false;
      this.mergeToolId = (await this.#store.get<string>("mergeToolId")) ?? "";
      this.mergeToolCmd = (await this.#store.get<string>("mergeToolCmd")) ?? "";
      this.logLevel = (await this.#store.get<string>("logLevel")) ?? "";
      this.recentFeatures = (await this.#store.get<string[]>("recentFeatures")) ?? [];
      this.gitPath = (await this.#store.get<string>("gitPath")) ?? "";
      this.settingsWidth = (await this.#store.get<number>("settingsWidth")) ?? 0;
      this.settingsHeight = (await this.#store.get<number>("settingsHeight")) ?? 0;
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
      // 唯一密钥自动启用（未指定时）；失败不阻断启动。
      this.#autoPickSshKey().catch(() => {});
    }
  }

  /**
   * 启动兜底：未指定 SSH 密钥且 ~/.ssh 恰好只有一把可用私钥时自动启用。
   * 仅在无歧义（唯一候选）时生效：多密钥时选哪把都是猜测，且注入
   * `-i + IdentitiesOnly` 会屏蔽其余钥匙，宁缺毋滥。toast 提示、设置页
   * 可改；探测/写入失败静默跳过。
   */
  async #autoPickSshKey(): Promise<void> {
    if (this.sshKeyPath) return;
    const keys = await commands.sshKeyList();
    const usable = keys.filter((k) => k.private_path);
    if (usable.length !== 1) return;
    const path = usable[0].private_path!;
    await this.setSshKeyPath(path);
    const name = path.split(/[\\/]/).pop() ?? path;
    showToast("info", t("settings.ssh.autoPicked", { name }));
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

  async setFileHistoryListWidth(width: number): Promise<void> {
    this.fileHistoryListWidth = width;
    await this.#store?.set("fileHistoryListWidth", width);
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

  /** P10: change + persist the tracing filter (applies immediately). */
  async setLogLevel(level: string): Promise<void> {
    this.logLevel = level;
    await this.#store?.set("logLevel", level);
    try {
      await commands.appSetLogLevel(level);
    } catch {
      // Non-fatal in pure-browser dev mode.
    }
  }

  /** P10: custom git executable; persisted only (restart applies it). */
  async setGitPath(path: string): Promise<void> {
    this.gitPath = path;
    await this.#store?.set("gitPath", path);
  }

  /**
   * 设置活动 SSH 私钥（空 = 清除）；立即下发 runner（GIT_SSH_COMMAND 注入）。
   * 仅改 sshKeyPath，代理设置保持当前值。
   */
  async setSshKeyPath(path: string): Promise<void> {
    this.sshKeyPath = path;
    await this.#store?.set("sshKeyPath", path);
    await commands.appSetNetConfig({
      proxy_mode: this.proxyMode,
      proxy_url: this.proxyUrl || null,
      ssh_key_path: path || null,
    });
  }

  /** 设置中心弹窗尺寸（0 = 复位为默认）。 */
  async setSettingsSize(width: number, height: number): Promise<void> {
    this.settingsWidth = width;
    this.settingsHeight = height;
    await this.#store?.set("settingsWidth", width);
    await this.#store?.set("settingsHeight", height);
  }

  /** P10: MRU bookkeeping for the command palette. */
  async pushRecentFeature(id: string): Promise<void> {
    const next = [id, ...this.recentFeatures.filter((x) => x !== id)].slice(0, 10);
    this.recentFeatures = next;
    await this.#store?.set("recentFeatures", next);
  }

  async setEditorFont(font: string): Promise<void> {
    this.editorFont = font;
    await this.#store?.set("editorFont", font);
  }

  async setEditorTabSize(size: number): Promise<void> {
    this.editorTabSize = size;
    await this.#store?.set("editorTabSize", size);
  }

  async setPullStrategy(strategy: "merge" | "rebase" | "ff_only"): Promise<void> {
    this.pullStrategy = strategy;
    await this.#store?.set("pullStrategy", strategy);
  }

  async setPushSetUpstream(mode: "whenMissing" | "always" | "never"): Promise<void> {
    this.pushSetUpstream = mode;
    await this.#store?.set("pushSetUpstream", mode);
  }

  async setPushIncludeTags(v: boolean): Promise<void> {
    this.pushIncludeTags = v;
    await this.#store?.set("pushIncludeTags", v);
  }

  /** Default external merge tool selection ("" = git default). */
  async setMergeTool(id: string, cmd: string): Promise<void> {
    this.mergeToolId = id;
    this.mergeToolCmd = cmd;
    await this.#store?.set("mergeToolId", id);
    await this.#store?.set("mergeToolCmd", cmd);
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
