<script lang="ts">
  /**
   * 设置中心（P10）shell：左侧导航 + section 路由 + 几何。通用 / Git /
   * 网络 / 外部工具 / 高级内联在此；重状态面板拆分为 SshSection /
   * AiSection / GitConfigSection（首次激活挂载、激活时刷新）。
   */
  import * as Dialog from "$lib/components/ui/dialog";
  import { ResizableContent } from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { t } from "$lib/i18n";
  import { settings } from "$lib/stores/settings.svelte";
  import { appDialogs, type SettingsSection } from "$lib/stores/appdialogs.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import { app, normalizeError, type CommitTemplate } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import { appDataDir, join } from "@tauri-apps/api/path";
  import SshSection from "$lib/components/settings/SshSection.svelte";
  import AiSection from "$lib/components/settings/AiSection.svelte";
  import GitConfigSection from "$lib/components/settings/GitConfigSection.svelte";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import Globe from "@lucide/svelte/icons/globe";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import Wrench from "@lucide/svelte/icons/wrench";
  import FolderGit2 from "@lucide/svelte/icons/folder-git-2";
  import SlidersHorizontal from "@lucide/svelte/icons/sliders-horizontal";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Sparkles from "@lucide/svelte/icons/sparkles";

  const open = $derived(appDialogs.settingsOpen);
  const section = $derived(appDialogs.settingsSection);

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

  // ---- commit template（git section）----
  let template = $state<CommitTemplate | null>(null);

  const activeRepo = $derived(repos.active);

  // 重状态面板：open && section 激活时各自加载数据（同旧中央 effect 语义）。
  const credActive = $derived(open && section === "credentials");
  const aiActive = $derived(open && section === "ai");
  const gitconfigActive = $derived(open && section === "gitconfig");

  $effect(() => {
    if (!open) return;
    sshKeyPath = settings.sshKeyPath;
    proxyMode = settings.proxyMode;
    proxyUrl = settings.proxyUrl;
    gitPathInput = settings.gitPath;
    gitPathResult = null;
    if (section === "git") void loadTemplate();
  });


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
  <!-- 几何/握把/持久化由 ResizableContent 承担；持久化 0 = 恢复默认 -->
  <ResizableContent
    open={open}
    defaultWidth={880}
    defaultHeight={620}
    minWidth={560}
    minHeight={360}
    savedWidth={settings.settingsWidth}
    savedHeight={settings.settingsHeight}
    labels={{
      width: t("settings.resize.width"),
      height: t("settings.resize.height"),
      both: t("settings.resize.both"),
    }}
    onPersist={(w, h) => void settings.setSettingsSize(w, h)}
    onReset={() => void settings.setSettingsSize(0, 0)}
    class="flex gap-0 overflow-hidden p-0"
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

      <!-- 重状态面板：保持挂载（hidden）以保留未保存草稿，激活时各自刷新 -->
      <div class={section !== "credentials" ? "hidden" : ""}>
        <SshSection active={credActive} />
      </div>
      <div class={section !== "ai" ? "hidden" : ""}>
        <AiSection active={aiActive} />
      </div>
      <div class={section !== "gitconfig" ? "hidden" : ""}>
        <GitConfigSection active={gitconfigActive} />
      </div>
    </div>
  </ResizableContent>
</Dialog.Root>
