<script lang="ts">
  /**
   * Settings center (P10)：通用 / Git / 网络 / 凭据 / 外部工具 / Git 配置 /
   * 高级。P7 的网络与凭据管理从 NetworkDialog 迁入；mergetool 默认工具
   * （P8 遗留的持久化）也在此配置。
   */
  import * as Dialog from "$lib/components/ui/dialog";
  import { untrack } from "svelte";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { t } from "$lib/i18n";
  import { settings } from "$lib/stores/settings.svelte";
  import { appDialogs, type SettingsSection } from "$lib/stores/appdialogs.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import {
    app,
    net,
    ai,
    normalizeError,
    type CommitTemplate,
    type ConfigEntry,
    type GitignoreFile,
    type AiConfigDto,
    type SshKeyInfo,
  } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import { ConfirmDialog } from "$lib/components/ui/confirm-dialog";
  import { appDataDir, join } from "@tauri-apps/api/path";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import Globe from "@lucide/svelte/icons/globe";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import Wrench from "@lucide/svelte/icons/wrench";
  import FolderGit2 from "@lucide/svelte/icons/folder-git-2";
  import SlidersHorizontal from "@lucide/svelte/icons/sliders-horizontal";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Sparkles from "@lucide/svelte/icons/sparkles";
  import Lock from "@lucide/svelte/icons/lock";
  import Copy from "@lucide/svelte/icons/copy";
  import Plus from "@lucide/svelte/icons/plus";
  import Eye from "@lucide/svelte/icons/eye";

  const open = $derived(appDialogs.settingsOpen);
  const section = $derived(appDialogs.settingsSection);

  // ---- 弹窗几何：右缘/底缘/右下角握把自由拖拽调大小，左上角锚定，尺寸持久化 ----
  const DLG_DEFAULT_W = 880;
  const DLG_DEFAULT_H = 620;
  const DLG_MIN_W = 560;
  const DLG_MIN_H = 360;
  const DLG_MARGIN = 12; // 与主窗口边缘的最小间距

  type DlgDir = "e" | "s" | "se";
  let dlgW = $state(DLG_DEFAULT_W);
  let dlgH = $state(DLG_DEFAULT_H);
  let dlgX = $state(0);
  let dlgY = $state(0);
  let resizeDir = $state<DlgDir | null>(null);

  function clampDlgSize(w: number, h: number): { w: number; h: number } {
    const maxW = Math.max(DLG_MIN_W, window.innerWidth - DLG_MARGIN * 2);
    const maxH = Math.max(DLG_MIN_H, window.innerHeight - DLG_MARGIN * 2);
    return {
      w: Math.min(maxW, Math.max(DLG_MIN_W, Math.round(w))),
      h: Math.min(maxH, Math.max(DLG_MIN_H, Math.round(h))),
    };
  }

  function centerDlg(w: number, h: number): void {
    dlgX = Math.max(0, Math.round((window.innerWidth - w) / 2));
    dlgY = Math.max(0, Math.round((window.innerHeight - h) / 2));
  }

  // 打开时恢复上次尺寸并居中；$effect.pre 渲染前执行，避免首帧位置闪跳。
  // untrack：拖拽提交的新尺寸不应触发重新居中。
  $effect.pre(() => {
    if (!open) return;
    untrack(() => {
      const c = clampDlgSize(
        settings.settingsWidth || DLG_DEFAULT_W,
        settings.settingsHeight || DLG_DEFAULT_H,
      );
      dlgW = c.w;
      dlgH = c.h;
      centerDlg(c.w, c.h);
    });
  });

  // 主窗口缩小时把弹窗钳回可视范围。
  $effect(() => {
    if (!open) return;
    const onViewportResize = (): void => {
      const c = clampDlgSize(dlgW, dlgH);
      dlgW = c.w;
      dlgH = c.h;
      dlgX = Math.min(dlgX, Math.max(0, window.innerWidth - c.w));
      dlgY = Math.min(dlgY, Math.max(0, window.innerHeight - c.h));
    };
    window.addEventListener("resize", onViewportResize);
    return () => window.removeEventListener("resize", onViewportResize);
  });

  function beginResize(dir: DlgDir, event: PointerEvent): void {
    if (event.button !== 0) return;
    event.preventDefault();
    resizeDir = dir;
    const startX = event.clientX;
    const startY = event.clientY;
    const startW = dlgW;
    const startH = dlgH;
    (event.currentTarget as Element).setPointerCapture(event.pointerId);

    const onMove = (ev: PointerEvent): void => {
      const c = clampDlgSize(
        dir === "s" ? startW : startW + (ev.clientX - startX),
        dir === "e" ? startH : startH + (ev.clientY - startY),
      );
      dlgW = c.w;
      dlgH = c.h;
    };
    const finish = (): void => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      resizeDir = null;
      void settings.setSettingsSize(dlgW, dlgH);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
  }

  /** 握把聚焦后方向键微调，步进即持久化。 */
  function resizeKey(dir: DlgDir, event: KeyboardEvent): void {
    const dx = event.key === "ArrowLeft" ? -8 : event.key === "ArrowRight" ? 8 : 0;
    const dy = event.key === "ArrowUp" ? -8 : event.key === "ArrowDown" ? 8 : 0;
    if (dx === 0 && dy === 0) return;
    event.preventDefault();
    const c = clampDlgSize(
      dir === "s" ? dlgW : dlgW + dx,
      dir === "e" ? dlgH : dlgH + dy,
    );
    dlgW = c.w;
    dlgH = c.h;
    void settings.setSettingsSize(c.w, c.h);
  }

  /** 双击角部握把：恢复默认尺寸并重新居中（持久化 0 = 默认）。 */
  function resetDlgSize(): void {
    const c = clampDlgSize(DLG_DEFAULT_W, DLG_DEFAULT_H);
    dlgW = c.w;
    dlgH = c.h;
    centerDlg(c.w, c.h);
    void settings.setSettingsSize(0, 0);
  }

  const SECTIONS: { id: SettingsSection; icon: typeof Settings2 }[] = [
    { id: "general", icon: Settings2 },
    { id: "git", icon: GitBranch },
    { id: "network", icon: Globe },
    { id: "credentials", icon: KeyRound },
    { id: "ai", icon: Sparkles },
    { id: "tools", icon: Wrench },
    { id: "gitconfig", icon: FolderGit2 },
    { id: "advanced", icon: SlidersHorizontal },
  ];

  // ---- form mirrors ----
  let sshKeyPath = $state("");
  let proxyMode = $state<"inherit" | "none" | "custom">("inherit");
  let proxyUrl = $state("");

  // ---- git path test ----
  let gitPathInput = $state("");
  let gitPathTesting = $state(false);
  let gitPathResult = $state<string | null>(null);

  // ---- SSH 密钥管理（~/.ssh 列出 / 生成 / 删除） ----
  let sshKeys = $state<SshKeyInfo[]>([]);
  let sshLoading = $state(false);
  // 生成对话框
  let genOpen = $state(false);
  let genAlgo = $state<"ed25519" | "rsa4096">("ed25519");
  let genFileName = $state("");
  let genComment = $state("");
  let genPass = $state("");
  let genPass2 = $state("");
  let genBusy = $state(false);
  // 删除确认 / 公钥查看
  let deleteTarget = $state<SshKeyInfo | null>(null);
  let deleteOpen = $state(false);
  let pubOpen = $state(false);
  let pubView = $state<SshKeyInfo | null>(null);

  // ---- git config viewer ----
  let globalConfig = $state<ConfigEntry[]>([]);
  let localConfig = $state<ConfigEntry[]>([]);
  let gitignore = $state<GitignoreFile | null>(null);
  let template = $state<CommitTemplate | null>(null);
  let configLoading = $state(false);

  // ---- git config 常用项编辑（写入全局配置）----
  // blur 即时保存：非空 = set，空 = unset；与 general 区字体/缩进的交互一致。
  const COMMON_CONFIG_KEYS = [
    { key: "user.name", labelKey: "settings.gitconfig.userName", placeholder: "IbexGit" },
    { key: "user.email", labelKey: "settings.gitconfig.userEmail", placeholder: "you@example.com" },
    { key: "http.proxy", labelKey: "settings.gitconfig.httpProxy", placeholder: "http://127.0.0.1:7890" },
    { key: "https.proxy", labelKey: "settings.gitconfig.httpsProxy", placeholder: "http://127.0.0.1:7890" },
  ] as const;
  let cfgEffective = $state<Record<string, string>>({});
  let cfgDrafts = $state<Record<string, string>>(
    Object.fromEntries(COMMON_CONFIG_KEYS.map((f) => [f.key, ""])),
  );
  let cfgSaving = $state<string | null>(null);

  // ---- AI (P11) ----
  let aiCfg = $state<AiConfigDto | null>(null);
  let aiKeyInput = $state("");
  let aiTesting = $state(false);
  let aiTestResult = $state<string | null>(null);
  let aiSavingKey = $state(false);

  const activeRepo = $derived(repos.active);

  $effect(() => {
    if (!open) return;
    sshKeyPath = settings.sshKeyPath;
    proxyMode = settings.proxyMode;
    proxyUrl = settings.proxyUrl;
    gitPathInput = settings.gitPath;
    gitPathResult = null;
    if (section === "credentials") void loadSshKeys();
    if (section === "gitconfig") void loadConfig();
    if (section === "git") void loadTemplate();
    if (section === "ai") void loadAiConfig();
  });

  async function loadAiConfig(): Promise<void> {
    try {
      aiCfg = await ai.configGet();
      aiKeyInput = "";
      aiTestResult = null;
    } catch (err) {
      normalizeError(err);
    }
  }

  function aiPatch(patch: Partial<AiConfigDto>): void {
    if (aiCfg) aiCfg = { ...aiCfg, ...patch };
  }

  async function saveAiConfig(): Promise<void> {
    if (!aiCfg) return;
    try {
      // 非空输入才更新密钥；空 = 保持不变（清除走独立按钮）。
      if (aiKeyInput.trim()) {
        aiSavingKey = true;
        await ai.setKey(aiKeyInput.trim());
        aiKeyInput = "";
        showToast("success", t("ai.settings.keySaved"));
      }
      aiCfg = await ai.configSet(aiCfg);
      showToast("success", t("ai.settings.saved"));
    } catch (err) {
      normalizeError(err);
    } finally {
      aiSavingKey = false;
    }
  }

  async function deleteAiKey(): Promise<void> {
    try {
      await ai.deleteKey();
      aiCfg = aiCfg ? { ...aiCfg, has_key: false } : aiCfg;
      showToast("success", t("ai.settings.keyDeleted"));
    } catch (err) {
      normalizeError(err);
    }
  }

  async function testAiConnection(): Promise<void> {
    if (!aiCfg) return;
    aiTesting = true;
    aiTestResult = null;
    try {
      // 先落密钥再落配置，has_key 随返回值刷新（清除/占位提示同步）。
      if (aiKeyInput.trim()) {
        await ai.setKey(aiKeyInput.trim());
        aiKeyInput = "";
      }
      aiCfg = await ai.configSet(aiCfg);
      aiTestResult = await ai.testConnection();
    } catch (err) {
      normalizeError(err);
      aiTestResult = null;
    } finally {
      aiTesting = false;
    }
  }

  const AI_PROVIDERS = [
    { id: "ollama", labelKey: "ai.settings.ollama", base: "http://localhost:11434/v1" },
    { id: "open_ai_compatible", labelKey: "ai.settings.openaiCompatible", base: "https://api.openai.com/v1" },
    { id: "anthropic", labelKey: "ai.settings.anthropic", base: "https://api.anthropic.com" },
    { id: "custom", labelKey: "ai.settings.custom", base: "" },
  ] as const;

  function setAiProvider(id: (typeof AI_PROVIDERS)[number]["id"]): void {
    if (!aiCfg) return;
    const preset = AI_PROVIDERS.find((p) => p.id === id);
    aiPatch({
      provider: id,
      base_url: preset?.base ?? "",
    });
  }

  function addCurrentRepoToExcluded(): void {
    const path = repos.active?.path;
    if (!path || !aiCfg) return;
    if (!aiCfg.excluded_repos.includes(path)) {
      aiPatch({ excluded_repos: [...aiCfg.excluded_repos, path] });
    }
  }

  // =====================
  // SSH 密钥管理
  // =====================

  async function loadSshKeys(): Promise<void> {
    sshLoading = true;
    try {
      sshKeys = await net.sshKeyList();
    } catch (err) {
      normalizeError(err);
    } finally {
      sshLoading = false;
    }
  }

  function openGenerate(): void {
    genAlgo = "ed25519";
    genFileName = "";
    genComment = "";
    genPass = "";
    genPass2 = "";
    genOpen = true;
  }

  function keyFileName(key: SshKeyInfo): string {
    return key.public_path.split(/[\\/]/).pop() ?? key.public_path;
  }

  async function generateSshKey(): Promise<void> {
    genBusy = true;
    try {
      const info = await net.sshKeyGenerate({
        algorithm:
          genAlgo === "ed25519" ? { kind: "ed25519" } : { kind: "rsa", bits: 4096 },
        comment: genComment.trim() || null,
        passphrase: genPass || null,
        file_name: genFileName.trim() || null,
      });
      genOpen = false;
      // 首把密钥自动设为活动：未指定过密钥时生成即启用（此刻意图最明确，
      // 已有指定则不覆盖；活动密钥可随时在列表一键切换/撤销）。
      if (!settings.sshKeyPath && info.private_path) {
        await settings.setSshKeyPath(info.private_path);
        showToast("success", t("settings.ssh.generatedActive", { name: keyFileName(info) }));
      } else {
        showToast("success", t("settings.ssh.generated", { name: keyFileName(info) }));
      }
      await loadSshKeys();
    } catch (err) {
      normalizeError(err);
    } finally {
      genBusy = false;
    }
  }

  function setActiveKey(key: SshKeyInfo): void {
    if (!key.private_path) return;
    settings
      .setSshKeyPath(key.private_path)
      .then(() => showToast("success", t("settings.ssh.activeSet")))
      .catch(normalizeError);
  }

  /** 取消活动密钥（空 = 不再注入 GIT_SSH_COMMAND，认证交回系统 ssh 配置）。 */
  function clearActiveKey(): void {
    settings
      .setSshKeyPath("")
      .then(() => showToast("info", t("settings.ssh.activeUnset")))
      .catch(normalizeError);
  }

  async function confirmDeleteKey(): Promise<void> {
    const key = deleteTarget;
    deleteTarget = null;
    if (!key) return;
    try {
      await net.sshKeyDelete(key.private_path ?? key.public_path);
      if (key.private_path && settings.sshKeyPath === key.private_path) {
        // 删除的正是活动密钥：清空并停用注入。
        await settings.setSshKeyPath("");
        showToast("info", t("settings.ssh.activeCleared"));
      }
      showToast("success", t("settings.ssh.deleted", { name: keyFileName(key) }));
      await loadSshKeys();
    } catch (err) {
      normalizeError(err);
    }
  }

  function copyPubKey(key: SshKeyInfo): void {
    navigator.clipboard
      ?.writeText(key.public_key)
      .then(() => showToast("success", t("settings.ssh.copied")))
      .catch(() => {});
  }

  async function loadConfig(): Promise<void> {
    configLoading = true;
    try {
      const [g, ig] = await Promise.all([app.configGlobal(), app.gitignoreGlobal()]);
      globalConfig = g;
      gitignore = ig;
      localConfig = activeRepo ? await app.configLocal(activeRepo.id) : [];
      syncCommonConfig(g);
    } catch (err) {
      normalizeError(err);
    } finally {
      configLoading = false;
    }
  }

  /** 同名键以最后一次出现为准（git 语义），key 比较忽略大小写。 */
  function syncCommonConfig(entries: ConfigEntry[]): void {
    const eff: Record<string, string> = {};
    for (const e of entries) {
      const lower = e.key.toLowerCase();
      if (COMMON_CONFIG_KEYS.some((f) => f.key === lower)) eff[lower] = e.value;
    }
    cfgEffective = eff;
    // 保存中的字段不动草稿，避免 reload 覆盖用户正在输入的内容。
    for (const f of COMMON_CONFIG_KEYS) {
      if (cfgSaving !== f.key) cfgDrafts[f.key] = eff[f.key] ?? "";
    }
  }

  async function saveCommonConfig(key: string): Promise<void> {
    const draft = (cfgDrafts[key] ?? "").trim();
    if (draft === (cfgEffective[key] ?? "")) return; // 未变化（含未设置过）
    cfgSaving = key;
    try {
      await app.configSetGlobal(key, draft === "" ? null : draft);
      cfgDrafts[key] = draft;
      showToast("success", t("settings.gitconfig.saved"));
      await loadConfig();
    } catch (err) {
      normalizeError(err);
      await loadConfig();
      cfgDrafts[key] = cfgEffective[key] ?? ""; // 回滚到真实值
    } finally {
      cfgSaving = null;
    }
  }

  async function loadTemplate(): Promise<void> {
    template = null;
    if (!activeRepo) return;
    try {
      template = await app.commitTemplate(activeRepo.id);
    } catch {
      template = null; // unset or unreadable → no template UI
    }
  }

  async function saveNetConfig(): Promise<void> {
    try {
      await settings.setNetwork({ sshKeyPath, proxyMode, proxyUrl });
      showToast("success", t("net.configSaved"));
    } catch (err) {
      normalizeError(err);
    }
  }

  async function testGitPath(): Promise<void> {
    gitPathTesting = true;
    gitPathResult = null;
    try {
      gitPathResult = await app.checkGitPath(gitPathInput);
    } catch (err) {
      normalizeError(err);
      gitPathResult = null;
    } finally {
      gitPathTesting = false;
    }
  }

  async function saveGitPath(): Promise<void> {
    await settings.setGitPath(gitPathInput.trim());
    showToast("info", t("settings.git.pathSaved"));
  }

  async function openDir(sub: string | null): Promise<void> {
    try {
      const base = await appDataDir();
      const target = sub ? await join(base, sub) : base;
      // 不走 opener 插件：Windows 上其对目录是 SHOpenFolderAndSelectItems
      // 的"父窗口中选中"语义，且父窗口已存在时静默无动作（同 Toolbar）。
      // app_open_folder 用 explorer/open/xdg-open 真正进入目录。
      await app.openFolder(target);
    } catch (err) {
      normalizeError(err);
    }
  }

  const MERGE_TOOLS = [
    { id: "", label: "settings.tools.gitDefault" },
    { id: "vscode", label: "settings.tools.vscode" },
    { id: "meld", label: "settings.tools.meld" },
    { id: "kdiff3", label: "settings.tools.kdiff3" },
    { id: "p4merge", label: "settings.tools.p4merge" },
    { id: "vimdiff", label: "settings.tools.vimdiff" },
    { id: "custom", label: "settings.tools.custom" },
  ] as const;

  const LOG_LEVELS = ["trace", "debug", "info", "warn", "error"] as const;

  function setMergeTool(id: string): void {
    const cmd = id === "custom" ? settings.mergeToolCmd : "";
    void settings.setMergeTool(id, cmd);
  }

  function setCustomCmd(cmd: string): void {
    void settings.setMergeTool(settings.mergeToolId, cmd);
  }
</script>

<Dialog.Root bind:open={appDialogs.settingsOpen}>
  <!-- 尺寸/位置走 inline style：可靠压过基类的 sm:max-w-sm 与居中 transform -->
  <Dialog.Content
    class="flex gap-0 overflow-hidden p-0 {resizeDir ? 'select-none' : ''}"
    style="left:{dlgX}px; top:{dlgY}px; width:{dlgW}px; height:{dlgH}px; max-width:none; translate:none;"
  >
    <Dialog.Header class="sr-only">
      <Dialog.Title>{t("settings.title")}</Dialog.Title>
      <Dialog.Description>{t("settings.desc")}</Dialog.Description>
    </Dialog.Header>

    <!-- left nav -->
    <nav class="flex w-44 shrink-0 flex-col gap-0.5 overflow-y-auto border-r bg-muted/30 p-2">
      {#each SECTIONS as s (s.id)}
        <button
          type="button"
          class="flex items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[13px] {section ===
          s.id
            ? 'bg-accent font-medium text-accent-foreground'
            : 'text-muted-foreground hover:bg-accent/60'}"
          onclick={() => appDialogs.openSettings(s.id)}
        >
          <s.icon class="size-3.5" />
          {t(`settings.nav.${s.id}`)}
        </button>
      {/each}
    </nav>

    <!-- right pane（高度由弹窗决定，随握把伸缩） -->
    <div class="min-h-0 min-w-0 flex-1 overflow-y-auto p-4">
      {#if section === "general"}
        <section class="space-y-4">
          <h3 class="text-sm font-semibold">{t("settings.nav.general")}</h3>
          <div class="space-y-1.5">
            <span class="text-xs text-muted-foreground">{t("settings.general.theme")}</span>
            <div class="grid grid-cols-3 gap-1.5">
              {#each ["light", "dark", "system"] as mode}
                <button
                  type="button"
                  class="rounded-md border px-2 py-1.5 text-[12px] transition-colors {settings.theme ===
                  mode
                    ? 'border-primary bg-primary/10 text-foreground'
                    : 'text-muted-foreground hover:bg-accent/50'}"
                  onclick={() => void settings.setTheme(mode as typeof settings.theme)}
                >
                  {t(`theme.${mode}`)}
                </button>
              {/each}
            </div>
          </div>
          <div class="space-y-1.5">
            <span class="text-xs text-muted-foreground">{t("settings.general.language")}</span>
            <div class="grid grid-cols-2 gap-1.5">
              {#each ["zh-CN", "en"] as loc}
                <button
                  type="button"
                  class="rounded-md border px-2 py-1.5 text-[12px] transition-colors {settings.locale ===
                  loc
                    ? 'border-primary bg-primary/10 text-foreground'
                    : 'text-muted-foreground hover:bg-accent/50'}"
                  onclick={() => void settings.setLocale(loc as typeof settings.locale)}
                >
                  {t(loc === "zh-CN" ? "lang.zhCN" : "lang.en")}
                </button>
              {/each}
            </div>
          </div>
          <label class="block space-y-1">
            <span class="text-xs text-muted-foreground">{t("settings.general.font")}</span>
            <Input
              value={settings.editorFont}
              placeholder={t("settings.general.fontHint")}
              class="h-8 font-mono text-[12px]"
              onchange={(e) => void settings.setEditorFont(e.currentTarget.value)}
            />
          </label>
          <label class="block space-y-1">
            <span class="text-xs text-muted-foreground">{t("settings.general.tabSize")}</span>
            <Input
              type="number"
              min="1"
              max="8"
              value={settings.editorTabSize}
              class="h-8 w-24"
              onchange={(e) =>
                void settings.setEditorTabSize(
                  Math.min(8, Math.max(1, Number(e.currentTarget.value) || 4)),
                )}
            />
          </label>
        </section>
      {:else if section === "git"}
        <section class="space-y-4">
          <h3 class="text-sm font-semibold">{t("settings.nav.git")}</h3>

          <div class="space-y-1.5">
            <span class="text-xs text-muted-foreground">{t("settings.git.pullStrategy")}</span>
            <div class="grid grid-cols-3 gap-1.5">
              {#each ["merge", "rebase", "ff_only"] as m}
                <button
                  type="button"
                  class="rounded-md border px-2 py-1.5 text-[12px] transition-colors {settings.pullStrategy ===
                  m
                    ? 'border-primary bg-primary/10 text-foreground'
                    : 'text-muted-foreground hover:bg-accent/50'}"
                  onclick={() => void settings.setPullStrategy(m as typeof settings.pullStrategy)}
                >
                  {t(`refs.pull.mode${m === "ff_only" ? "FfOnly" : m === "rebase" ? "Rebase" : "Merge"}`)}
                </button>
              {/each}
            </div>
            <p class="text-[11px] text-muted-foreground">{t("settings.git.pullHint")}</p>
          </div>

          <div class="space-y-1.5">
            <span class="text-xs text-muted-foreground">{t("settings.git.pushUpstream")}</span>
            <div class="grid grid-cols-3 gap-1.5">
              {#each ["whenMissing", "always", "never"] as m}
                <button
                  type="button"
                  class="rounded-md border px-2 py-1.5 text-[12px] transition-colors {settings.pushSetUpstream ===
                  m
                    ? 'border-primary bg-primary/10 text-foreground'
                    : 'text-muted-foreground hover:bg-accent/50'}"
                  onclick={() => void settings.setPushSetUpstream(m as typeof settings.pushSetUpstream)}
                >
                  {t(`settings.git.upstream_${m}`)}
                </button>
              {/each}
            </div>
            <label class="flex items-center gap-2 pt-1 text-[13px]">
              <Checkbox
                checked={settings.pushIncludeTags}
                onCheckedChange={(v) => void settings.setPushIncludeTags(v === true)}
              />
              {t("settings.git.pushTags")}
            </label>
          </div>

          <div class="space-y-1.5 rounded-md border p-3">
            <span class="text-xs font-medium">{t("settings.git.path")}</span>
            <div class="flex gap-1.5">
              <Input
                bind:value={gitPathInput}
                placeholder="git"
                class="h-8 flex-1 font-mono text-[12px]"
              />
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={gitPathTesting || !gitPathInput.trim()}
                onclick={() => void testGitPath()}
              >
                {#if gitPathTesting}<LoaderCircle class="size-3.5 animate-spin" />{/if}
                {t("settings.git.pathTest")}
              </Button>
            </div>
            {#if gitPathResult}
              <p class="font-mono text-[11px] text-success dark:text-success">{gitPathResult}</p>
            {/if}
            <div class="flex items-center justify-between">
              <p class="text-[11px] text-muted-foreground">{t("settings.git.pathHint")}</p>
              <Button type="button" size="sm" variant="outline" onclick={() => void saveGitPath()}>
                {t("settings.git.pathSave")}
              </Button>
            </div>
          </div>

          <div class="space-y-1.5 rounded-md border p-3">
            <span class="text-xs font-medium">{t("settings.git.template")}</span>
            {#if !activeRepo}
              <p class="text-[11px] text-muted-foreground">{t("settings.git.needRepo")}</p>
            {:else if template === null}
              <p class="text-[11px] text-muted-foreground">{t("settings.git.templateNone")}</p>
            {:else}
              <p class="font-mono text-[11px] text-muted-foreground">{template.path}</p>
              <pre
                class="editor-font max-h-28 overflow-auto rounded border bg-muted/40 p-2 text-[11px] whitespace-pre-wrap">{template.content}</pre>
            {/if}
          </div>
        </section>
      {:else if section === "network"}
        <section class="space-y-3">
          <h3 class="text-sm font-semibold">{t("settings.nav.network")}</h3>
          <label class="block space-y-1">
            <span class="text-xs text-muted-foreground">{t("net.sshKey")}</span>
            <Input
              bind:value={sshKeyPath}
              placeholder={t("net.sshKeyHint")}
              class="h-8 font-mono text-[12px]"
            />
          </label>
          <div class="grid grid-cols-3 gap-1.5">
            {#each ["inherit", "none", "custom"] as mode}
              <button
                type="button"
                class="rounded-md border px-2 py-1.5 text-[12px] transition-colors {proxyMode ===
                mode
                  ? 'border-primary bg-primary/10 text-foreground'
                  : 'text-muted-foreground hover:bg-accent/50'}"
                onclick={() => (proxyMode = mode as typeof proxyMode)}
              >
                {t(`net.proxy_${mode}`)}
              </button>
            {/each}
          </div>
          {#if proxyMode === "custom"}
            <label class="block space-y-1">
              <span class="text-xs text-muted-foreground">{t("net.proxyUrl")}</span>
              <Input
                bind:value={proxyUrl}
                placeholder="http://127.0.0.1:7890"
                class="h-8 font-mono text-[12px]"
              />
            </label>
          {:else if proxyMode === "none"}
            <label class="flex items-center gap-2 text-[13px]">
              <Checkbox checked={true} disabled />
              <span class="text-muted-foreground">{t("net.proxyNoneHint")}</span>
            </label>
          {/if}
          <div class="flex justify-end">
            <Button type="button" size="sm" onclick={() => void saveNetConfig()}>
              {t("net.configSave")}
            </Button>
          </div>
        </section>
      {:else if section === "credentials"}
        <section class="space-y-4">
          <div class="flex items-center justify-between">
            <h3 class="text-sm font-semibold">{t("settings.nav.credentials")}</h3>
            <div class="flex items-center gap-2">
              {#if sshLoading}<LoaderCircle class="size-3.5 animate-spin" />{/if}
              <Button type="button" size="sm" onclick={openGenerate}>
                <Plus class="size-3.5" data-icon="inline-start" />
                {t("settings.ssh.generate")}
              </Button>
            </div>
          </div>
          <p class="text-[11px] text-muted-foreground">{t("settings.ssh.dirHint")}</p>

          {#if sshKeys.length === 0}
            <p class="rounded-md border border-dashed px-3 py-6 text-center text-xs text-muted-foreground">
              {t("settings.ssh.empty")}
            </p>
          {:else}
            <ul class="divide-y overflow-hidden rounded-md border">
              {#each sshKeys as key (key.public_path)}
                <li class="flex items-center gap-3 px-3 py-2.5">
                  <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-1.5">
                      <span class="truncate text-[13px] font-medium">
                        {key.comment || keyFileName(key)}
                      </span>
                      {#if key.encrypted}
                        <Lock class="size-3 shrink-0 text-muted-foreground" aria-label={t("settings.ssh.encrypted")} />
                      {/if}
                      {#if key.private_path && settings.sshKeyPath === key.private_path}
                        <span class="shrink-0 rounded bg-primary/10 px-1.5 py-0.5 text-[10px] font-medium text-primary">
                          {t("settings.ssh.active")}
                        </span>
                      {/if}
                    </div>
                    <div class="truncate font-mono text-[11px] text-muted-foreground" title={key.public_path}>
                      {key.algorithm}{key.bits > 0 ? ` · ${key.bits}` : ""} · {key.fingerprint} · {key.public_path}
                    </div>
                  </div>
                  <div class="flex shrink-0 items-center gap-0.5">
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      title={t("settings.ssh.viewPub")}
                      onclick={() => {
                        pubView = key;
                        pubOpen = true;
                      }}
                    >
                      <Eye class="size-3.5 text-muted-foreground" />
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      title={t("settings.ssh.copyPub")}
                      onclick={() => copyPubKey(key)}
                    >
                      <Copy class="size-3.5 text-muted-foreground" />
                    </Button>
                    {#if key.private_path && settings.sshKeyPath !== key.private_path}
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        title={t("settings.ssh.setActiveHint")}
                        onclick={() => setActiveKey(key)}
                      >
                        {t("settings.ssh.setActive")}
                      </Button>
                    {:else if key.private_path}
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        title={t("settings.ssh.unsetActiveHint")}
                        onclick={clearActiveKey}
                      >
                        {t("settings.ssh.unsetActive")}
                      </Button>
                    {/if}
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      title={t("settings.ssh.deleteKey")}
                      onclick={() => {
                        deleteTarget = key;
                        deleteOpen = true;
                      }}
                    >
                      <Trash2 class="size-3.5 text-muted-foreground" />
                    </Button>
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
          <p class="text-[11px] text-muted-foreground">{t("settings.ssh.activeHint")}</p>
        </section>
      {:else if section === "ai"}
        <section class="space-y-4">
          <h3 class="text-sm font-semibold">{t("settings.nav.ai")}</h3>
          <p class="text-[11px] text-muted-foreground">{t("ai.settings.desc")}</p>

          {#if aiCfg}
            <label class="flex items-start gap-2 rounded-md border p-3">
              <Checkbox
                checked={aiCfg.enabled}
                onCheckedChange={(v) => aiPatch({ enabled: v === true })}
              />
              <span class="space-y-0.5">
                <span class="block text-[13px] font-medium">{t("ai.settings.enable")}</span>
                <span class="block text-[11px] text-muted-foreground">{t("ai.settings.enableHint")}</span>
              </span>
            </label>

            <div class="space-y-1.5">
              <span class="text-xs text-muted-foreground">{t("ai.settings.provider")}</span>
              <div class="grid grid-cols-4 gap-1.5">
                {#each AI_PROVIDERS as p (p.id)}
                  <button
                    type="button"
                    class="rounded-md border px-2 py-1.5 text-[11px] transition-colors {aiCfg.provider ===
                    p.id
                      ? 'border-primary bg-primary/10 text-foreground'
                      : 'text-muted-foreground hover:bg-accent/50'}"
                    onclick={() => setAiProvider(p.id)}
                  >
                    {t(p.labelKey)}
                  </button>
                {/each}
              </div>
              {#if aiCfg.provider === "ollama"}
                <p class="text-[11px] text-success dark:text-success">{t("ai.settings.ollamaHint")}</p>
              {/if}
            </div>

            <div class="grid grid-cols-2 gap-3">
              <label class="block space-y-1">
                <span class="text-xs text-muted-foreground">
                  {aiCfg.provider === "custom" ? t("ai.settings.baseUrlCustom") : t("ai.settings.baseUrl")}
                </span>
                <Input
                  value={aiCfg.base_url}
                  placeholder={aiCfg.provider === "custom"
                    ? "https://gateway.example.com/v1/chat/completions"
                    : "http://localhost:11434/v1"}
                  class="h-8 font-mono text-[12px]"
                  onchange={(e) => aiPatch({ base_url: e.currentTarget.value })}
                />
              </label>
              <label class="block space-y-1">
                <span class="text-xs text-muted-foreground">{t("ai.settings.model")}</span>
                <Input
                  value={aiCfg.model}
                  placeholder={t("ai.settings.modelHint")}
                  class="h-8 font-mono text-[12px]"
                  onchange={(e) => aiPatch({ model: e.currentTarget.value })}
                />
              </label>
            </div>

            <div class="grid grid-cols-2 gap-3">
              <div class="space-y-1">
                <span class="text-xs text-muted-foreground">{t("ai.settings.key")}</span>
                <div class="flex gap-1.5">
                  <Input
                    type="password"
                    bind:value={aiKeyInput}
                    placeholder={aiCfg.has_key ? t("ai.settings.keySet") : t("ai.settings.keyPlaceholder")}
                    class="h-8 flex-1 font-mono text-[12px]"
                    autocomplete="off"
                  />
                  {#if aiCfg.has_key}
                    <Button type="button" variant="outline" size="sm" onclick={() => void deleteAiKey()}>
                      {t("ai.settings.keyDelete")}
                    </Button>
                  {/if}
                </div>
                {#if aiCfg.provider === "custom"}
                  <select
                    value={aiCfg.custom_auth}
                    class="h-8 w-full rounded-md border bg-background px-2 text-[12px]"
                    onchange={(e) => aiPatch({ custom_auth: e.currentTarget.value })}
                  >
                    <option value="bearer">Authorization: Bearer</option>
                    <option value="x_api_key">x-api-key</option>
                    <option value="none">—</option>
                  </select>
                {/if}
              </div>
              <label class="block space-y-1">
                <span class="text-xs text-muted-foreground">{t("ai.settings.timeout")}</span>
                <!-- value 必须与 option 的 __value 同为 number：Svelte 用
                     Object.is(option.__value, value) 匹配选中项，传 String 会
                     永远失配 → selectedIndex=-1 不回显 -->
                <select
                  value={aiCfg.timeout_secs}
                  class="h-8 w-full rounded-md border bg-background px-2 text-[13px]"
                  onchange={(e) => aiPatch({ timeout_secs: Number(e.currentTarget.value) })}
                >
                  {#if ![30, 60, 120, 300, 600].includes(aiCfg.timeout_secs)}
                    <option value={aiCfg.timeout_secs}>{aiCfg.timeout_secs}</option>
                  {/if}
                  {#each [30, 60, 120, 300, 600] as s (s)}
                    <option value={s}>{s}</option>
                  {/each}
                </select>
              </label>
            </div>

            <label class="flex items-center gap-2 text-[13px]">
              <Checkbox
                checked={aiCfg.conventional}
                onCheckedChange={(v) => aiPatch({ conventional: v === true })}
              />
              {t("ai.settings.conventional")}
            </label>

            <div class="space-y-1.5 rounded-md border p-3">
              <span class="text-xs font-medium">{t("ai.settings.privacyTitle")}</span>
              <label class="block space-y-1">
                <span class="text-xs text-muted-foreground">{t("ai.settings.excludePatterns")}</span>
                <textarea
                  rows={4}
                  value={aiCfg.exclude_patterns.join("\n")}
                  class="w-full resize-y rounded-md border border-input bg-transparent px-3 py-2 font-mono text-[11px] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                  onchange={(e) =>
                    aiPatch({
                      exclude_patterns: e.currentTarget.value
                        .split("\n")
                        .map((s) => s.trim())
                        .filter((s) => s.length > 0),
                    })}
                ></textarea>
                <p class="text-[11px] text-muted-foreground">{t("ai.settings.excludeHint")}</p>
              </label>
              <div class="space-y-1">
                <div class="flex items-center justify-between">
                  <span class="text-xs text-muted-foreground">{t("ai.settings.excludedRepos")}</span>
                  {#if repos.active}
                    <Button type="button" variant="ghost" size="sm" onclick={addCurrentRepoToExcluded}>
                      {t("ai.settings.addCurrentRepo")}
                    </Button>
                  {:else}
                    <span class="text-[11px] text-muted-foreground">{t("ai.settings.noRepo")}</span>
                  {/if}
                </div>
                {#if aiCfg.excluded_repos.length > 0}
                  <ul class="divide-y overflow-hidden rounded-md border">
                    {#each aiCfg.excluded_repos as rp (rp)}
                      <li class="flex items-center gap-2 px-2.5 py-1.5">
                        <span class="min-w-0 flex-1 truncate font-mono text-[11px]" title={rp}>{rp}</span>
                        <button
                          type="button"
                          class="text-muted-foreground hover:text-foreground"
                          onclick={() =>
                            aiPatch({ excluded_repos: aiCfg!.excluded_repos.filter((x) => x !== rp) })}
                          aria-label={t("ai.settings.keyDelete")}
                        >
                          <Trash2 class="size-3.5" />
                        </button>
                      </li>
                    {/each}
                  </ul>
                {/if}
                <p class="text-[11px] text-muted-foreground">{t("ai.settings.excludedReposHint")}</p>
              </div>
            </div>

            <div class="flex items-center gap-2">
              <Button type="button" variant="outline" size="sm" disabled={aiTesting} onclick={() => void testAiConnection()}>
                {#if aiTesting}<LoaderCircle class="size-3.5 animate-spin" data-icon="inline-start" />{/if}
                {t("ai.settings.test")}
              </Button>
              {#if aiTestResult}
                <span class="truncate font-mono text-[11px] text-success dark:text-success">{aiTestResult}</span>
              {/if}
            </div>

            <div class="flex justify-end">
              <Button type="button" size="sm" disabled={aiSavingKey} onclick={() => void saveAiConfig()}>
                {t("ai.settings.save")}
              </Button>
            </div>
          {:else}
            <p class="text-[11px] text-muted-foreground"><LoaderCircle class="mr-1 inline size-3 animate-spin" />…</p>
          {/if}
        </section>
      {:else if section === "tools"}
        <section class="space-y-4">
          <h3 class="text-sm font-semibold">{t("settings.nav.tools")}</h3>
          <div class="space-y-1.5">
            <span class="text-xs text-muted-foreground">{t("settings.tools.default")}</span>
            <div class="grid grid-cols-3 gap-1.5">
              {#each MERGE_TOOLS as tool}
                <button
                  type="button"
                  class="rounded-md border px-2 py-1.5 text-[12px] transition-colors {settings.mergeToolId ===
                  tool.id
                    ? 'border-primary bg-primary/10 text-foreground'
                    : 'text-muted-foreground hover:bg-accent/50'}"
                  onclick={() => setMergeTool(tool.id)}
                >
                  {t(tool.label)}
                </button>
              {/each}
            </div>
            <p class="text-[11px] text-muted-foreground">{t("settings.tools.hint")}</p>
          </div>
          {#if settings.mergeToolId === "custom"}
            <label class="block space-y-1">
              <span class="text-xs text-muted-foreground">{t("settings.tools.cmd")}</span>
              <Input
                value={settings.mergeToolCmd}
                placeholder="code --wait --merge $REMOTE $LOCAL $BASE $MERGED"
                class="h-8 font-mono text-[12px]"
                onchange={(e) => setCustomCmd(e.currentTarget.value)}
              />
            </label>
          {/if}
        </section>
      {:else if section === "gitconfig"}
        <section class="space-y-5">
          <div class="flex items-center justify-between">
            <h3 class="text-sm font-semibold">{t("settings.nav.gitconfig")}</h3>
            {#if configLoading}<LoaderCircle class="size-3.5 animate-spin" />{/if}
          </div>
          <p class="text-[11px] text-muted-foreground">{t("settings.gitconfig.readonly")}</p>

          <div class="space-y-1.5 rounded-md border p-3">
            <span class="text-xs font-medium">{t("settings.gitconfig.common")}</span>
            <div class="grid grid-cols-2 gap-x-3 gap-y-2">
              {#each COMMON_CONFIG_KEYS as f (f.key)}
                <label class="block space-y-1">
                  <span class="text-xs text-muted-foreground">{t(f.labelKey)}</span>
                  <Input
                    bind:value={cfgDrafts[f.key]}
                    placeholder={f.placeholder}
                    class="h-8 font-mono text-[12px]"
                    disabled={cfgSaving === f.key}
                    onchange={() => void saveCommonConfig(f.key)}
                  />
                </label>
              {/each}
            </div>
            <p class="text-[11px] text-muted-foreground">{t("settings.gitconfig.commonHint")}</p>
          </div>

          <div class="space-y-1.5">
            <h4 class="text-xs font-semibold text-muted-foreground">{t("settings.gitconfig.global")}</h4>
            {#if globalConfig.length === 0}
              <p class="rounded-md border border-dashed px-3 py-2 text-center text-xs text-muted-foreground">
                {t("settings.gitconfig.empty")}
              </p>
            {:else}
              <ul class="max-h-44 divide-y overflow-y-auto rounded-md border">
                {#each globalConfig as entry, i (`${entry.key}.${i}`)}
                  <li class="flex gap-2 px-2.5 py-1.5 font-mono text-[11px]">
                    <span class="w-44 shrink-0 truncate text-muted-foreground" title={entry.key}>{entry.key}</span>
                    <span class="min-w-0 flex-1 whitespace-pre-wrap break-all">{entry.value}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>

          <div class="space-y-1.5">
            <h4 class="text-xs font-semibold text-muted-foreground">{t("settings.gitconfig.local")}</h4>
            {#if !activeRepo}
              <p class="text-[11px] text-muted-foreground">{t("settings.git.needRepo")}</p>
            {:else if localConfig.length === 0}
              <p class="rounded-md border border-dashed px-3 py-2 text-center text-xs text-muted-foreground">
                {t("settings.gitconfig.empty")}
              </p>
            {:else}
              <ul class="max-h-44 divide-y overflow-y-auto rounded-md border">
                {#each localConfig as entry, i (`${entry.key}.${i}`)}
                  <li class="flex gap-2 px-2.5 py-1.5 font-mono text-[11px]">
                    <span class="w-44 shrink-0 truncate text-muted-foreground" title={entry.key}>{entry.key}</span>
                    <span class="min-w-0 flex-1 whitespace-pre-wrap break-all">{entry.value}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>

          <div class="space-y-1.5">
            <h4 class="text-xs font-semibold text-muted-foreground">{t("settings.gitconfig.gitignore")}</h4>
            {#if gitignore === null}
              <p class="text-[11px] text-muted-foreground">{t("settings.gitconfig.gitignoreNone")}</p>
            {:else}
              <p class="font-mono text-[11px] text-muted-foreground">{gitignore.path}</p>
              <pre
                class="editor-font max-h-36 overflow-auto rounded border bg-muted/40 p-2 text-[11px]">{gitignore.content}</pre>
            {/if}
          </div>
        </section>
      {:else if section === "advanced"}
        <section class="space-y-4">
          <h3 class="text-sm font-semibold">{t("settings.nav.advanced")}</h3>
          <label class="block w-40 space-y-1">
            <span class="text-xs text-muted-foreground">{t("settings.advanced.logLevel")}</span>
            <select
              value={settings.logLevel || ""}
              class="h-8 w-full rounded-md border bg-background px-2 text-[13px]"
              onchange={(e) => void settings.setLogLevel(e.currentTarget.value)}
            >
              <option value="">{t("settings.advanced.logDefault")}</option>
              {#each LOG_LEVELS as lvl}
                <option value={lvl}>{lvl}</option>
              {/each}
            </select>
            <p class="text-[11px] text-muted-foreground">{t("settings.advanced.logHint")}</p>
          </label>
          <div class="flex gap-2">
            <Button type="button" variant="outline" size="sm" onclick={() => void openDir(null)}>
              <FolderOpen class="size-3.5" data-icon="inline-start" />
              {t("settings.advanced.openConfig")}
            </Button>
            <Button type="button" variant="outline" size="sm" onclick={() => void openDir("logs")}>
              <FolderOpen class="size-3.5" data-icon="inline-start" />
              {t("settings.advanced.openLogs")}
            </Button>
          </div>
        </section>
      {/if}
    </div>

    <!-- 自由调大小握把：右缘 / 底缘 / 右下角（左上角锚定；角部双击复位） -->
    <button
      type="button"
      aria-label={t("settings.resize.width")}
      class="absolute inset-y-0 right-0 z-10 w-1.5 cursor-ew-resize touch-none bg-transparent transition-colors outline-none hover:bg-primary/20 focus-visible:bg-primary/20 {resizeDir === 'e' ? 'bg-primary/30' : ''}"
      onpointerdown={(e) => beginResize("e", e)}
      onkeydown={(e) => resizeKey("e", e)}
    ></button>
    <button
      type="button"
      aria-label={t("settings.resize.height")}
      class="absolute inset-x-0 bottom-0 z-10 h-1.5 cursor-ns-resize touch-none bg-transparent transition-colors outline-none hover:bg-primary/20 focus-visible:bg-primary/20 {resizeDir === 's' ? 'bg-primary/30' : ''}"
      onpointerdown={(e) => beginResize("s", e)}
      onkeydown={(e) => resizeKey("s", e)}
    ></button>
    <button
      type="button"
      aria-label={t("settings.resize.both")}
      class="absolute right-0 bottom-0 z-20 size-4 cursor-nwse-resize touch-none bg-transparent transition-colors outline-none hover:bg-primary/20 focus-visible:bg-primary/20 {resizeDir === 'se' ? 'bg-primary/30' : ''}"
      onpointerdown={(e) => beginResize("se", e)}
      ondblclick={resetDlgSize}
      onkeydown={(e) => resizeKey("se", e)}
    ></button>
  </Dialog.Content>
</Dialog.Root>

<!-- SSH 密钥生成（嵌套在设置中心之上） -->
<Dialog.Root bind:open={genOpen}>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>{t("settings.ssh.genTitle")}</Dialog.Title>
      <Dialog.Description>{t("settings.ssh.genDesc")}</Dialog.Description>
    </Dialog.Header>
    <div class="space-y-3">
      <div class="space-y-1.5">
        <span class="text-xs text-muted-foreground">{t("settings.ssh.algorithm")}</span>
        <div class="grid grid-cols-2 gap-1.5">
          {#each [["ed25519", "settings.ssh.algoEd25519", "settings.ssh.algoEd25519Hint"], ["rsa4096", "settings.ssh.algoRsa", "settings.ssh.algoRsaHint"]] as [id, label, hint]}
            <button
              type="button"
              class="rounded-md border px-2 py-1.5 text-left text-[12px] transition-colors {genAlgo === id
                ? 'border-primary bg-primary/10 text-foreground'
                : 'text-muted-foreground hover:bg-accent/50'}"
              onclick={() => (genAlgo = id as typeof genAlgo)}
            >
              {t(label)}
              <span class="block text-[10px] opacity-80">{t(hint)}</span>
            </button>
          {/each}
        </div>
      </div>
      <div class="grid grid-cols-2 gap-3">
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("settings.ssh.fileName")}</span>
          <Input
            bind:value={genFileName}
            placeholder={genAlgo === "ed25519" ? "id_ed25519" : "id_rsa"}
            class="h-8 font-mono text-[12px]"
          />
        </label>
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("settings.ssh.comment")}</span>
          <Input
            bind:value={genComment}
            placeholder="you@example.com"
            class="h-8 text-[12px]"
          />
        </label>
      </div>
      <div class="grid grid-cols-2 gap-3">
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("settings.ssh.passphrase")}</span>
          <Input
            type="password"
            bind:value={genPass}
            placeholder={t("settings.ssh.passphraseHint")}
            class="h-8 font-mono text-[12px]"
            autocomplete="new-password"
          />
        </label>
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("settings.ssh.passphraseConfirm")}</span>
          <Input
            type="password"
            bind:value={genPass2}
            class="h-8 font-mono text-[12px]"
            autocomplete="new-password"
          />
        </label>
      </div>
      {#if genPass !== genPass2}
        <p class="text-[11px] text-destructive">{t("settings.ssh.passMismatch")}</p>
      {/if}
      <p class="text-[11px] text-muted-foreground">{t("settings.ssh.passphraseNote")}</p>
    </div>
    <Dialog.Footer>
      <Button type="button" variant="ghost" size="sm" disabled={genBusy} onclick={() => (genOpen = false)}>
        {t("common.cancel")}
      </Button>
      <Button
        type="button"
        size="sm"
        disabled={genBusy || (genPass.length > 0 && genPass !== genPass2)}
        onclick={() => void generateSshKey()}
      >
        {#if genBusy}<LoaderCircle class="size-3.5 animate-spin" data-icon="inline-start" />{/if}
        {t("settings.ssh.generate")}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- 公钥查看 / 复制 -->
<Dialog.Root bind:open={pubOpen}>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{t("settings.ssh.pubTitle")}</Dialog.Title>
      {#if pubView}
        <Dialog.Description class="font-mono text-[11px]">
          {pubView.algorithm}{pubView.bits > 0 ? ` · ${pubView.bits}` : ""} · {pubView.fingerprint}
        </Dialog.Description>
      {/if}
    </Dialog.Header>
    {#if pubView}
      <pre
        class="editor-font max-h-40 overflow-auto rounded border bg-muted/40 p-2 text-[11px] break-all whitespace-pre-wrap">{pubView.public_key}</pre>
    {/if}
    <Dialog.Footer>
      <Button type="button" variant="ghost" size="sm" onclick={() => (pubOpen = false)}>
        {t("common.close")}
      </Button>
      {#if pubView}
        <Button type="button" size="sm" onclick={() => copyPubKey(pubView!)}>
          <Copy class="size-3.5" data-icon="inline-start" />
          {t("settings.ssh.copyPub")}
        </Button>
      {/if}
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- 删除确认（破坏性操作：私钥 + 公钥同时移除，不可恢复） -->
<ConfirmDialog
  bind:open={deleteOpen}
  title={deleteTarget ? t("settings.ssh.deleteTitle", { name: keyFileName(deleteTarget) }) : ""}
  description={t("settings.ssh.deleteDesc")}
  confirmLabel={t("settings.ssh.deleteKey")}
  destructive
  onconfirm={() => void confirmDeleteKey()}
/>

