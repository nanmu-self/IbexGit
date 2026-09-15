<script lang="ts">
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { t, setLocale } from "$lib/i18n";
  import { keymap, formatBinding, getPlatform, type ActionId } from "$lib/keyboard/keymap";
  import { emitAction } from "$lib/keyboard";
  import { settings } from "$lib/stores/settings.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import { showToast } from "$lib/stores/toast";
  import { pickRepo } from "$lib/repo-picker";
  import { getVersion } from "@tauri-apps/api/app";
  import Sun from "@lucide/svelte/icons/sun";
  import Moon from "@lucide/svelte/icons/moon";
  import Monitor from "@lucide/svelte/icons/monitor";

  let aboutOpen = $state(false);
  let shortcutsOpen = $state(false);
  let version = $state("");

  const platform = getPlatform();

  function shortcutLabel(action: ActionId): string {
    const binding = keymap[action];
    return binding ? formatBinding(binding, platform) : "";
  }

  async function openAbout(): Promise<void> {
    try {
      version = await getVersion();
    } catch {
      version = "?";
    }
    aboutOpen = true;
  }

  async function cycleTheme(): Promise<void> {
    const order = ["light", "dark", "system"] as const;
    const next = order[(order.indexOf(settings.theme) + 1) % order.length];
    await settings.setTheme(next);
  }
</script>

<header
  class="flex h-9 shrink-0 items-center gap-0.5 border-b bg-background px-2 text-[13px] select-none"
>
  <div class="mr-3 flex items-center gap-1.5 pl-1 font-semibold">
    <span
      class="flex size-5 items-center justify-center rounded bg-primary text-[9px] font-bold text-primary-foreground"
    >
      IG
    </span>
    <span>{t("app.name")}</span>
  </div>

  <!-- 文件 -->
  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({ props })}
        <button {...props} class="rounded px-2.5 py-1 hover:bg-accent">{t("menu.file")}</button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="start" class="w-52">
      <DropdownMenu.Item onSelect={pickRepo}>
        {t("menu.file.openRepo")}
        <DropdownMenu.Shortcut>{shortcutLabel("repo.open")}</DropdownMenu.Shortcut>
      </DropdownMenu.Item>
      <DropdownMenu.Separator />
      <DropdownMenu.Item disabled onSelect={() => {}}>{t("menu.file.clone")}</DropdownMenu.Item>
      <DropdownMenu.Item disabled onSelect={() => {}}>{t("menu.file.newRepo")}</DropdownMenu.Item>
      <DropdownMenu.Separator />
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
      <DropdownMenu.Item onSelect={() => emitAction("view.toggleSidebar")}>
        {t("menu.view.toggleSidebar")}
        <DropdownMenu.Shortcut>{shortcutLabel("view.toggleSidebar")}</DropdownMenu.Shortcut>
      </DropdownMenu.Item>
      <DropdownMenu.Item onSelect={() => emitAction("view.toggleTheme")}>
        {t("menu.view.toggleTheme")}
        <DropdownMenu.Shortcut>{shortcutLabel("view.toggleTheme")}</DropdownMenu.Shortcut>
      </DropdownMenu.Item>
      <DropdownMenu.Separator />
      <DropdownMenu.Item onSelect={() => emitAction("repo.refresh")}>
        {t("menu.view.refresh")}
        <DropdownMenu.Shortcut>{shortcutLabel("repo.refresh")}</DropdownMenu.Shortcut>
      </DropdownMenu.Item>
    </DropdownMenu.Content>
  </DropdownMenu.Root>

  <!-- 仓库 -->
  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({ props })}
        <button {...props} class="rounded px-2.5 py-1 hover:bg-accent">{t("menu.repository")}</button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="start" class="w-52">
      <DropdownMenu.Item disabled onSelect={() => {}}>
        {t("menu.repo.fetch")}
      </DropdownMenu.Item>
      <DropdownMenu.Item disabled onSelect={() => {}}>
        {t("menu.repo.pull")}
      </DropdownMenu.Item>
      <DropdownMenu.Item disabled onSelect={() => {}}>
        {t("menu.repo.push")}
      </DropdownMenu.Item>
      <DropdownMenu.Separator />
      <DropdownMenu.Item disabled onSelect={() => {}}>
        {t("menu.repo.newBranch")}
      </DropdownMenu.Item>
    </DropdownMenu.Content>
  </DropdownMenu.Root>

  <!-- 语言 -->
  <DropdownMenu.Root>
    <DropdownMenu.Trigger>
      {#snippet child({ props })}
        <button {...props} class="rounded px-2.5 py-1 hover:bg-accent">{t("menu.language")}</button>
      {/snippet}
    </DropdownMenu.Trigger>
    <DropdownMenu.Content align="start" class="w-40">
      <DropdownMenu.RadioGroup
        value={settings.locale}
        onValueChange={(v) => settings.setLocale(v as "zh-CN" | "en")}
      >
        <DropdownMenu.RadioItem value="zh-CN">{t("lang.zhCN")}</DropdownMenu.RadioItem>
        <DropdownMenu.RadioItem value="en">{t("lang.en")}</DropdownMenu.RadioItem>
      </DropdownMenu.RadioGroup>
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
      <DropdownMenu.Item onSelect={() => (shortcutsOpen = true)}>
        {t("menu.help.shortcuts")}
      </DropdownMenu.Item>
      <DropdownMenu.Item onSelect={openAbout}>{t("menu.help.about")}</DropdownMenu.Item>
    </DropdownMenu.Content>
  </DropdownMenu.Root>

  <div class="ml-auto flex items-center gap-1 pr-1">
    <Button
      variant="ghost"
      size="icon"
      class="size-7"
      onclick={cycleTheme}
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
  </div>
</header>

<!-- 关于 -->
<Dialog.Root bind:open={aboutOpen}>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>{t("dialog.about.title")}</Dialog.Title>
      <Dialog.Description>
        {t("dialog.about.desc", { name: t("app.name"), version })}
      </Dialog.Description>
    </Dialog.Header>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (aboutOpen = false)}>{t("common.close")}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- 快捷键 -->
<Dialog.Root bind:open={shortcutsOpen}>
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
      <Button variant="outline" onclick={() => (shortcutsOpen = false)}>
        {t("common.close")}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
