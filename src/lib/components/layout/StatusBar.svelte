<script lang="ts">
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { emitAction } from "$lib/keyboard";
  import { netops } from "$lib/stores/netops.svelte";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import ListTodo from "@lucide/svelte/icons/list-todo";
  import Inbox from "@lucide/svelte/icons/inbox";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  const active = $derived(repos.active);
  const netOp = $derived(active ? (netops.busy[active.id] ?? null) : null);
</script>

<footer
  class="flex h-7 shrink-0 items-center gap-3 border-t bg-muted/40 px-3 text-[11px] text-muted-foreground"
>
  {#if active}
    <span class="flex items-center gap-1.5" title={active.path}>
      <GitBranch class="size-3" />
      <span class="max-w-48 truncate font-medium text-foreground/80">
        {active.branch || t("statusbar.detached")}
      </span>
    </span>
    {#if active.ahead > 0 || active.behind > 0}
      <span class="flex items-center gap-1.5 tabular-nums">
        {#if active.ahead > 0}<span class="text-info0">↑{active.ahead}</span>{/if}
        {#if active.behind > 0}<span class="text-danger0">↓{active.behind}</span>{/if}
      </span>
    {/if}
    {#if netOp}
      <!-- P6 进度提示: 网络操作进行中（Task 化进度待任务中心接入） -->
      <span class="flex items-center gap-1 text-muted-foreground">
        <LoaderCircle class="size-3 animate-spin" />
        {t(`netops.${netOp}Running`)}
      </span>
    {/if}
  {:else}
    <span>{t("statusbar.noRepo")}</span>
  {/if}

  <div class="ml-auto flex items-center gap-2">
    {#if active?.lastRefreshMs !== null && active?.lastRefreshMs !== undefined}
      <span title={t("statusbar.lastRefresh", { ms: active.lastRefreshMs.toFixed(0) })}>
        {active.lastRefreshMs.toFixed(0)}ms
      </span>
    {/if}

    <!-- 任务中心入口 -->
    <DropdownMenu.Root>
      <DropdownMenu.Trigger>
        {#snippet child({ props })}
          <button
            {...props}
            type="button"
            class="flex items-center gap-1 rounded px-1.5 py-0.5 hover:bg-accent hover:text-foreground"
            title="{t('statusbar.tasks')} (Ctrl+J)"
          >
            <ListTodo class="size-3.5" />
          </button>
        {/snippet}
      </DropdownMenu.Trigger>
      <DropdownMenu.Content align="end" class="w-64 p-0">
        <div class="border-b px-3 py-2 text-xs font-medium">{t("statusbar.tasks")}</div>
        <div class="flex flex-col items-center gap-1.5 px-4 py-6 text-center">
          <Inbox class="size-6 text-muted-foreground/40" />
          <div class="text-xs text-muted-foreground">{t("statusbar.tasksEmpty")}</div>
          <div class="text-[11px] text-muted-foreground/70">{t("statusbar.tasksEmptyHint")}</div>
        </div>
      </DropdownMenu.Content>
    </DropdownMenu.Root>

    <button
      type="button"
      class="flex items-center gap-1 rounded px-1.5 py-0.5 hover:bg-accent hover:text-foreground"
      onclick={() => {
        if (active) void repos.refresh(active.id);
      }}
      title="{t('statusbar.refresh')} (F5)"
    >
      <RefreshCw class="size-3 {active?.refreshing ? 'animate-spin' : ''}" />
    </button>
  </div>
</footer>
