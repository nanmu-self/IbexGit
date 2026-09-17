<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import * as Tooltip from "$lib/components/ui/tooltip";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { pickRepo } from "$lib/repo-picker";
  import { normalizeError, git, app } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import { runFetch, isBusy } from "$lib/stores/netops.svelte";
  import { requestRefAction } from "$lib/stores/refbus";
  import Download from "@lucide/svelte/icons/download";
  import ArrowDownToLine from "@lucide/svelte/icons/arrow-down-to-line";
  import ArrowUpFromLine from "@lucide/svelte/icons/arrow-up-from-line";
  import Archive from "@lucide/svelte/icons/archive";
  import GitBranchPlus from "@lucide/svelte/icons/git-branch-plus";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Terminal from "@lucide/svelte/icons/terminal";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  const active = $derived(repos.active);
  const netBusy = $derived(active ? isBusy(active.id) : false);

  function reveal(): void {
    const path = repos.active?.path;
    if (!path) return;
    // 进入项目目录本身（而非在父目录中选中它）。不走 opener 插件：
    // Windows 上其 openPath 对目录是"父窗口中选中"的 reveal 语义。
    app.openFolder(path).catch((e) => normalizeError(e));
  }

  async function openTerminal(): Promise<void> {
    const path = repos.active?.path;
    if (!path) return;
    try {
      await app.openTerminal(path);
    } catch (e) {
      normalizeError(e);
    }
  }

  async function stashAll(): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    try {
      await git.stashPush(id, null);
      showToast("success", t("refs.stash.pushed"));
      await repos.refresh(id);
    } catch (e) {
      normalizeError(e);
    }
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
            disabled={!active || netBusy}
            onclick={() => active && void runFetch(active.id, null)}
          >
            {#if netBusy}
              <LoaderCircle class="size-4 animate-spin" />
            {:else}
              <Download class="size-4" />
            {/if}
            {t("toolbar.fetch")}
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{t("toolbar.fetchTip")}</Tooltip.Content>
    </Tooltip.Root>

    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon"
            class="flex h-11 w-12 flex-col gap-0.5 text-[10px] font-normal"
            disabled={!active || netBusy}
            onclick={() => requestRefAction({ kind: "pull" })}
          >
            <ArrowDownToLine class="size-4" />
            {t("toolbar.pull")}
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{t("toolbar.pullTip")}</Tooltip.Content>
    </Tooltip.Root>

    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon"
            class="flex h-11 w-12 flex-col gap-0.5 text-[10px] font-normal"
            disabled={!active || netBusy}
            onclick={() => requestRefAction({ kind: "push" })}
          >
            <span class="relative">
              <ArrowUpFromLine class="size-4" />
              <!-- 待推送数（ahead of upstream），随每次 refresh 更新 -->
              {#if active && active.ahead > 0}
                <span
                  class="absolute -top-1.5 -right-2.5 flex min-w-3.5 justify-center rounded-full bg-primary px-1 text-[9px] leading-[14px] font-semibold text-primary-foreground tabular-nums"
                >
                  {active.ahead > 99 ? "99+" : active.ahead}
                </span>
              {/if}
            </span>
            {t("toolbar.push")}
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{t("toolbar.pushTip")}</Tooltip.Content>
    </Tooltip.Root>

    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon"
            class="flex h-11 w-12 flex-col gap-0.5 text-[10px] font-normal"
            disabled={!active}
            onclick={stashAll}
          >
            <Archive class="size-4" />
            {t("toolbar.stash")}
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{t("toolbar.stashTip")}</Tooltip.Content>
    </Tooltip.Root>

    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon"
            class="flex h-11 w-12 flex-col gap-0.5 text-[10px] font-normal"
            disabled={!active}
            onclick={() => requestRefAction({ kind: "newBranch" })}
          >
            <GitBranchPlus class="size-4" />
            {t("toolbar.newBranch")}
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{t("toolbar.newBranchTip")}</Tooltip.Content>
    </Tooltip.Root>

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
      {#if active}
        <span class="max-w-60 truncate text-[13px] font-medium">{active.name}</span>
        <span class="flex items-center gap-1 text-[11px] text-muted-foreground">
          <GitBranch class="size-3" />
          {active.branch || t("statusbar.detached")}
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
            class="flex h-11 w-auto min-w-12 flex-col gap-0.5 px-2 text-[10px] font-normal"
            disabled={!active}
            onclick={openTerminal}
          >
            <Terminal class="size-4" />
            {t("toolbar.openTerminal")}
          </Button>
        {/snippet}
      </Tooltip.Trigger>
      <Tooltip.Content>{t("toolbar.openTerminalTip")}</Tooltip.Content>
    </Tooltip.Root>

    <Tooltip.Root>
      <Tooltip.Trigger>
        {#snippet child({ props })}
          <Button
            {...props}
            variant="ghost"
            size="icon"
            class="flex h-11 w-auto min-w-12 flex-col gap-0.5 px-2 text-[10px] font-normal"
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
