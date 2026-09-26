<script lang="ts">
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { emitAction } from "$lib/keyboard";
  import { netops } from "$lib/stores/netops.svelte";
  import { tasks, isActive, wireTasks } from "$lib/stores/tasks.svelte";
  import { formatAppError } from "$lib/git";
  import type { Task } from "$lib/git/bindings";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import ListTodo from "@lucide/svelte/icons/list-todo";
  import Inbox from "@lucide/svelte/icons/inbox";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import CircleCheck from "@lucide/svelte/icons/circle-check";
  import CircleX from "@lucide/svelte/icons/circle-x";
  import CircleMinus from "@lucide/svelte/icons/circle-minus";
  import Clock from "@lucide/svelte/icons/clock";
  import Eraser from "@lucide/svelte/icons/eraser";
  import X from "@lucide/svelte/icons/x";

  const active = $derived(repos.active);
  const netOp = $derived(active ? (netops.busy[active.id] ?? null) : null);

  // 任务中心事件订阅 + 初始列表（幂等）。
  wireTasks();

  /** 未知 kind 兜底显示原始值（t() 缺 key 时返回 key 本身）。 */
  function taskKind(kind: string): string {
    const label = t(`task.kind.${kind}`);
    return label === `task.kind.${kind}` ? kind : label;
  }

  function repoLabel(repoId: string | null): string {
    if (!repoId) return "";
    return repos.tabs.find((tab) => tab.id === repoId)?.name ?? "";
  }

  function errorMessage(task: Task): string | null {
    return task.status === "failed" && task.error ? formatAppError(task.error) : null;
  }
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
    {#if netOp}
      <!-- 网络操作即时提示；同一操作在任务中心有完整生命周期条目 -->
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

    <!-- 任务中心（P12）：后端 TaskManager 事件驱动 -->
    <DropdownMenu.Root open={tasks.open} onOpenChange={(o) => (tasks.open = o)}>
      <DropdownMenu.Trigger>
        {#snippet child({ props })}
          <button
            {...props}
            type="button"
            class="relative flex items-center gap-1 rounded px-1.5 py-0.5 hover:bg-accent hover:text-foreground"
            title="{t('statusbar.tasks')} (Ctrl+J)"
          >
            <ListTodo class="size-3.5" />
            {#if tasks.activeCount > 0}
              <span
                class="absolute -right-1.5 -top-1 flex h-3 min-w-3 items-center justify-center rounded-full bg-primary px-0.5 text-[9px] font-medium leading-none text-primary-foreground tabular-nums"
              >
                {tasks.activeCount}
              </span>
            {/if}
          </button>
        {/snippet}
      </DropdownMenu.Trigger>
      <DropdownMenu.Content align="end" class="w-80 p-0">
        <div class="flex items-center justify-between border-b px-3 py-2">
          <span class="text-xs font-medium">{t("statusbar.tasks")}</span>
          {#if tasks.items.some((item) => !isActive(item.status))}
            <button
              type="button"
              class="flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground"
              onclick={() => void tasks.clearFinished()}
            >
              <Eraser class="size-3" />
              {t("task.clear")}
            </button>
          {/if}
        </div>

        {#if tasks.sorted.length === 0}
          <div class="flex flex-col items-center gap-1.5 px-4 py-6 text-center">
            <Inbox class="size-6 text-muted-foreground/40" />
            <div class="text-xs text-muted-foreground">{t("statusbar.tasksEmpty")}</div>
            <div class="text-[11px] text-muted-foreground/70">{t("statusbar.tasksEmptyHint")}</div>
          </div>
        {:else}
          <div class="max-h-80 overflow-y-auto p-1">
            {#each tasks.sorted as task (task.id)}
              <div class="flex items-start gap-2 rounded-md px-2 py-1.5 hover:bg-accent/50">
                <span class="mt-0.5 shrink-0">
                  {#if task.status === "running" || task.status === "cancelling"}
                    <LoaderCircle class="size-3.5 animate-spin text-info" />
                  {:else if task.status === "success"}
                    <CircleCheck class="size-3.5 text-success" />
                  {:else if task.status === "failed"}
                    <CircleX class="size-3.5 text-danger" />
                  {:else if task.status === "cancelled"}
                    <CircleMinus class="size-3.5 text-muted-foreground" />
                  {:else}
                    <Clock class="size-3.5 text-muted-foreground" />
                  {/if}
                </span>
                <div class="min-w-0 flex-1">
                  <div class="flex items-baseline gap-1.5">
                    <span class="truncate text-xs font-medium">{taskKind(task.kind)}</span>
                    {#if repoLabel(task.repo_id)}
                      <span class="truncate text-[10px] text-muted-foreground/70">
                        {repoLabel(task.repo_id)}
                      </span>
                    {/if}
                    <span class="ml-auto shrink-0 text-[10px] text-muted-foreground">
                      {t(`task.status.${task.status}`)}
                    </span>
                  </div>
                  {#if errorMessage(task)}
                    <div class="truncate text-[11px] text-danger" title={errorMessage(task)}>
                      {errorMessage(task)}
                    </div>
                  {:else if task.message}
                    <div class="truncate text-[11px] text-muted-foreground" title={task.message}>
                      {task.message}
                    </div>
                  {/if}
                  {#if task.status === "running" && task.progress > 0}
                    <div class="mt-1 h-0.5 w-full overflow-hidden rounded bg-muted">
                      <div
                        class="h-full bg-info transition-[width]"
                        style="width: {Math.min(task.progress, 100)}%"
                      ></div>
                    </div>
                  {/if}
                </div>
                {#if task.cancellable && isActive(task.status)}
                  <button
                    type="button"
                    class="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
                    title={t("task.cancel")}
                    onclick={() => tasks.cancel(task.id)}
                  >
                    <X class="size-3" />
                  </button>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
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
