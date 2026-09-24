<script lang="ts">
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { t } from "$lib/i18n";
  import { keymap, formatBinding, getPlatform, type ActionId } from "$lib/keyboard/keymap";
  import { settings } from "$lib/stores/settings.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import { appDialogs } from "$lib/stores/appdialogs.svelte";
  import {
    featuresForSection,
    sectionGroups,
    type FeatureSection,
  } from "$lib/features";
  import { getVersion } from "@tauri-apps/api/app";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Sun from "@lucide/svelte/icons/sun";
  import Moon from "@lucide/svelte/icons/moon";
  import Monitor from "@lucide/svelte/icons/monitor";
  import Settings2 from "@lucide/svelte/icons/settings-2";
  import Keyboard from "@lucide/svelte/icons/keyboard";
  import Minus from "@lucide/svelte/icons/minus";
  import Square from "@lucide/svelte/icons/square";
  import Copy from "@lucide/svelte/icons/copy";
  import X from "@lucide/svelte/icons/x";
  import ExternalLink from "@lucide/svelte/icons/external-link";
  import Mail from "@lucide/svelte/icons/mail";
  import Scale from "@lucide/svelte/icons/scale";
  import Copyright from "@lucide/svelte/icons/copyright";
  import Tag from "@lucide/svelte/icons/tag";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { updater } from "$lib/updater/useAppUpdater.svelte";

  const platform = getPlatform();

  // ---- 关于（帮助菜单）----
  const ABOUT_REPO_URL = "https://github.com/nanmu-self/IbexGit";
  const ABOUT_EMAIL_URL = "mailto:157884200@qq.com";

  // 纯浏览器 dev 下无 Tauri IPC，静默降级（与窗口控制一致）。
  async function openExternal(url: string): Promise<void> {
    try {
      await openUrl(url);
    } catch {
      /* 非 Tauri 环境 */
    }
  }


  // ---- 菜单（P10）：菜单项由 features 注册表生成，键位引用 keymap ----
  function shortcutLabel(action: ActionId | undefined): string {
    if (!action) return "";
    const binding = keymap[action];
    return binding ? formatBinding(binding, platform) : "";
  }

  // ---- 自绘窗口控制（tauri.conf.json 已关 decorations）----
  // 纯浏览器 dev 下无 Tauri IPC，所有调用失败都静默降级。
  const appWindow = getCurrentWindow();
  let maximized = $state(false);

  async function syncMaximized(): Promise<void> {
    try {
      maximized = await appWindow.isMaximized();
    } catch {
      /* 非 Tauri 环境 */
    }
  }

  // 窗口尺寸变化（拖拽区双击、Win+方向键贴靠）时同步最大化态
  $effect(() => {
    void syncMaximized();
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    appWindow
      .onResized(() => void syncMaximized())
      .then((un) => {
        if (cancelled) un();
        else unlisten = un;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  });

  // 关于对话框的版本号：挂载时取一次。
  // 注意 features.ts 的「关于」项直接调 appDialogs.openAbout()（只置 open 标志），
  // 不经过本组件，所以版本号不能放在打开动作里取，否则永远为空。
  let aboutVersion = $state("");

  $effect(() => {
    getVersion()
      .then((version) => (aboutVersion = version))
      .catch(() => (aboutVersion = "?"));
  });

  function minimizeWindow(): void {
    void appWindow.minimize().catch(() => {});
  }

  function toggleMaximizeWindow(): void {
    void appWindow.toggleMaximize().catch(() => {});
  }

  function closeWindow(): void {
    void appWindow.close().catch(() => {});
  }
</script>

{#snippet sectionItems(section: FeatureSection)}
  {#each sectionGroups(section) as group, gi (group)}
    {#if gi > 0}
      <DropdownMenu.Separator />
    {/if}
    {#if section === "file" && group === 1}
      <!-- 最近仓库（File 菜单专属分组） -->
      <DropdownMenu.Sub>
        <DropdownMenu.SubTrigger>{t("menu.file.recent")}</DropdownMenu.SubTrigger>
        <DropdownMenu.SubContent class="w-64">
          {#if repos.recent.length === 0}
            <DropdownMenu.Item disabled>{t("menu.file.recentEmpty")}</DropdownMenu.Item>
          {:else}
            {#each repos.recent as r (r.path)}
              <DropdownMenu.Item onSelect={() => repos.openPath(r.path)}>
                <span class="flex-1 truncate">{r.name}</span>
                <span class="ml-3 max-w-40 truncate text-xs text-muted-foreground">{r.path}</span>
              </DropdownMenu.Item>
            {/each}
          {/if}
        </DropdownMenu.SubContent>
      </DropdownMenu.Sub>
      <DropdownMenu.Separator />
    {/if}
    {#each featuresForSection(section).filter((f) => f.group === group) as f (f.id)}
      <DropdownMenu.Item
        onSelect={f.run}
        disabled={f.needsRepo && !repos.active}
      >
        {t(f.labelKey)}
        {#if shortcutLabel(f.shortcut)}
          <DropdownMenu.Shortcut>{shortcutLabel(f.shortcut)}</DropdownMenu.Shortcut>
        {/if}
      </DropdownMenu.Item>
    {/each}
  {/each}
{/snippet}

<header
  data-tauri-drag-region
  class="relative flex h-9 shrink-0 items-center gap-0.5 border-b bg-background px-2 text-[13px] select-none"
>
  {#if platform === "macos"}
    <!-- 原生红绿灯由系统绘制（tauri.macos.conf.json: decorations + Overlay），此处仅占位避免与菜单重叠 -->
    <div class="w-[70px] shrink-0" aria-hidden="true"></div>
  {/if}

  <div data-tauri-drag-region class="mr-3 flex items-center pl-1">
    <img data-tauri-drag-region src="/logo.svg" alt="" draggable="false" class="size-8" />
  </div>

  <!-- 应用名：绝对定位水平居中，不受左右两侧内容宽度影响 -->
  <div
    data-tauri-drag-region
    class="pointer-events-none absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 font-semibold"
  >
    {t("app.name")}
  </div>

  <!-- 文件 -->
  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({ props })}
        <button {...props} class="rounded px-2.5 py-1 hover:bg-accent">{t("menu.file")}</button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="start" class="w-52">
      {@render sectionItems("file")}
    </DropdownMenu.Content>
  </DropdownMenu.Root>

  <!-- 查看 -->
  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({ props })}
        <button {...props} class="rounded px-2.5 py-1 hover:bg-accent">{t("menu.view")}</button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="start" class="w-56">
      {@render sectionItems("view")}
    </DropdownMenu.Content>
  </DropdownMenu.Root>

  <!-- 仓库 -->
  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({ props })}
        <button {...props} class="rounded px-2.5 py-1 hover:bg-accent">{t("menu.repository")}</button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="start" class="w-56">
      {@render sectionItems("repository")}
    </DropdownMenu.Content>
  </DropdownMenu.Root>

  <!-- 帮助 -->
  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({ props })}
        <button {...props} class="rounded px-2.5 py-1 hover:bg-accent">{t("menu.help")}</button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="start" class="w-44">
      {@render sectionItems("help")}
    </DropdownMenu.Content>
  </DropdownMenu.Root>

  <div data-tauri-drag-region class="ml-auto flex h-full items-center">
    <Button
      variant="ghost"
      size="icon"
      class="size-7"
      onclick={() => {
        const order = ["light", "dark", "system"] as const;
        const next = order[(order.indexOf(settings.theme) + 1) % order.length];
        void settings.setTheme(next);
      }}
      title="{t('theme.toggle')} ({t(`theme.${settings.theme}`)})"
    >
      {#if settings.theme === "dark"}
        <Moon class="size-4" />
      {:else if settings.theme === "light"}
        <Sun class="size-4" />
      {:else}
        <Monitor class="size-4" />
      {/if}
    </Button>
    <Button
      variant="ghost"
      size="icon"
      class="size-7"
      onclick={() => appDialogs.openSettings()}
      title="{t('settings.title')} ({shortcutLabel('app.settings')})"
    >
      <Settings2 class="size-4" />
    </Button>
    <Button
      variant="ghost"
      size="icon"
      class="size-7"
      onclick={() => appDialogs.openShortcuts()}
      title={t('menu.help.shortcuts')}
    >
      <Keyboard class="size-4" />
    </Button>

    {#if platform !== "macos"}
      <!-- 窗口控制：最小化 / 最大化·还原 / 关闭 -->
      <div class="ml-1 flex h-full items-center">
      <Button
        variant="ghost"
        size="icon"
        class="h-full w-11 rounded-none"
        onclick={minimizeWindow}
        title={t("win.minimize")}
        aria-label={t("win.minimize")}
      >
        <Minus class="size-4" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-full w-11 rounded-none"
        onclick={toggleMaximizeWindow}
        title={maximized ? t("win.restore") : t("win.maximize")}
        aria-label={maximized ? t("win.restore") : t("win.maximize")}
      >
        {#if maximized}
          <Copy class="size-3.5" />
        {:else}
          <Square class="size-3.5" />
        {/if}
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-full w-11 rounded-none hover:bg-danger hover:text-danger-foreground"
        onclick={closeWindow}
        title={t("win.close")}
        aria-label={t("win.close")}
      >
        <X class="size-4" />
      </Button>
    </div>
    {/if}
  </div>
</header>

<!-- 关于 -->
<Dialog.Root bind:open={appDialogs.aboutOpen}>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>{t("dialog.about.title")}</Dialog.Title>
      <Dialog.Description>
        {t("dialog.about.desc", { name: t("app.name") })}
      </Dialog.Description>
    </Dialog.Header>
    <div class="space-y-0.5 text-sm">
      <div class="flex items-center gap-2 rounded-md px-2 py-1.5">
        <Tag class="size-4 shrink-0 text-muted-foreground" />
        <span class="shrink-0 text-muted-foreground">{t("dialog.about.version")}</span>
        <span class="ml-auto text-xs font-medium text-foreground">v{aboutVersion}</span>
      </div>
      <button
        type="button"
        class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-muted/50"
        onclick={() => void openExternal(ABOUT_REPO_URL)}
        title={ABOUT_REPO_URL}
      >
        <ExternalLink class="size-4 shrink-0 text-muted-foreground" />
        <span class="shrink-0 text-muted-foreground">{t("dialog.about.repo")}</span>
        <span class="ml-auto truncate text-xs font-medium text-foreground">
          {ABOUT_REPO_URL.replace("https://", "")}
        </span>
      </button>
      <button
        type="button"
        class="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-muted/50"
        onclick={() => void openExternal(ABOUT_EMAIL_URL)}
        title={t("dialog.about.email")}
      >
        <Mail class="size-4 shrink-0 text-muted-foreground" />
        <span class="shrink-0 text-muted-foreground">{t("dialog.about.email")}</span>
        <span class="ml-auto truncate text-xs font-medium text-foreground">
          157884200@qq.com
        </span>
      </button>
      <div class="flex items-center gap-2 rounded-md px-2 py-1.5">
        <Scale class="size-4 shrink-0 text-muted-foreground" />
        <span class="shrink-0 text-muted-foreground">{t("dialog.about.license")}</span>
        <span class="ml-auto text-xs font-medium text-foreground">Apache-2.0</span>
      </div>
      <div class="flex items-center gap-2 rounded-md px-2 py-1.5">
        <Copyright class="size-4 shrink-0 text-muted-foreground" />
        <span class="shrink-0 text-muted-foreground">{t("dialog.about.copyright")}</span>
        <span class="ml-auto text-xs font-medium text-foreground">© 楠木</span>
      </div>
    </div>
    <Dialog.Footer>
      <Button
        variant="outline"
        disabled={updater.status === "checking" || updater.hasUpdate}
        onclick={() => void updater.checkForUpdate({ silent: false })}
      >
        {updater.status === "checking" ? t("update.checking") : t("update.check")}
      </Button>
      <Button variant="outline" onclick={() => (appDialogs.aboutOpen = false)}>
        {t("common.close")}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- 快捷键 -->
<Dialog.Root bind:open={appDialogs.shortcutsOpen}>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>{t("dialog.shortcuts.title")}</Dialog.Title>
      <Dialog.Description>
        {platform === "macos" ? "⌘ Cmd · ⌥ Alt · ⌃ Ctrl · ⇧ Shift" : ""}
      </Dialog.Description>
    </Dialog.Header>
    <div class="max-h-80 space-y-1.5 overflow-y-auto pr-1">
      {#each Object.entries(keymap) as [action, binding] (action)}
        {#if binding}
          <div class="flex items-center justify-between rounded-md px-2 py-1.5 text-sm odd:bg-muted/50">
            <span>{t(`shortcut.${action}`)}</span>
            <span class="font-mono text-xs text-muted-foreground">
              {formatBinding(binding, platform)}
            </span>
          </div>
        {/if}
      {/each}
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (appDialogs.shortcutsOpen = false)}>
        {t("common.close")}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
