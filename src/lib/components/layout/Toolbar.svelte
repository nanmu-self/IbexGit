<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import * as Tooltip from "$lib/components/ui/tooltip";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { pickRepo } from "$lib/repo-picker";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { normalizeError } from "$lib/git";
  import Download from "@lucide/svelte/icons/download";
  import ArrowDownToLine from "@lucide/svelte/icons/arrow-down-to-line";
  import ArrowUpFromLine from "@lucide/svelte/icons/arrow-up-from-line";
  import Archive from "@lucide/svelte/icons/archive";
  import GitBranchPlus from "@lucide/svelte/icons/git-branch-plus";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Terminal from "@lucide/svelte/icons/terminal";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import GitBranch from "@lucide/svelte/icons/git-branch";

  function reveal(): void {
    const path = repos.active?.path;
    if (!path) return;
    revealItemInDir(path).catch((e) => normalizeError(e));
  }
</script>

<Tooltip.Provider delayDuration={400}>
  <div class="flex h-14 shrink-0 items-center gap-0.5 border-b bg-background px-2">
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon"
            class="flex h-11 w-12 flex-col gap-0.5 text-[10px] font-normal"
            disabled
          >
            <Download class="size-4" />
            {t("toolbar.fetch")}
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{t("sidebar.comingSoon", { phase: "P6" })}</Tooltip.Content>
    </Tooltip.Root>

    {#each [{ icon: ArrowDownToLine, label: t("toolbar.pull") }, { icon: ArrowUpFromLine, label: t("toolbar.push") }, { icon: Archive, label: t("toolbar.stash") }, { icon: GitBranchPlus, label: t("toolbar.newBranch") }] as entry (entry.label)}
      <Button
        variant="ghost"
        size="icon"
        class="flex h-11 w-12 flex-col gap-0.5 text-[10px] font-normal"
        disabled
        title={t("sidebar.comingSoon", { phase: "P6" })}
      >
        <entry.icon class="size-4" />
        {entry.label}
      </Button>
    {/each}

    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon"
            class="flex h-11 w-12 flex-col gap-0.5 text-[10px] font-normal"
            onclick={pickRepo}
          >
            <FolderPlus class="size-4" />
            {t("toolbar.openRepo")}
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{t("tabs.open")}</Tooltip.Content>
    </Tooltip.Root>

    <!-- 中间：当前仓库名 + 分支 -->
    <div class="mx-4 flex min-w-0 flex-1 flex-col items-center justify-center leading-tight">
      {#if repos.active}
        <span class="max-w-60 truncate text-[13px] font-medium">{repos.active.name}</span>
        <span class="flex items-center gap-1 text-[11px] text-muted-foreground">
          <GitBranch class="size-3" />
          {repos.active.branch || t("statusbar.detached")}
        </span>
      {/if}
    </div>

    <!-- 右侧 -->
    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon"
            class="flex h-11 w-12 flex-col gap-0.5 text-[10px] font-normal"
            disabled
          >
            <Terminal class="size-4" />
            {t("toolbar.openTerminal")}
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{t("sidebar.comingSoon", { phase: "P6" })}</Tooltip.Content>
    </Tooltip.Root>

    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon"
            class="flex h-11 w-12 flex-col gap-0.5 text-[10px] font-normal"
            disabled={!repos.active}
            onclick={reveal}
          >
            <FolderOpen class="size-4" />
            {t("toolbar.showInExplorer")}
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{t("toolbar.showInExplorer")}</Tooltip.Content>
    </Tooltip.Root>
  </div>
</Tooltip.Provider>
