<script lang="ts">
  // P12 提交统计对话框：分支下拉 + 四个粒度 tab（总览/本月/本周/本日）
  // + 贡献者列表 + 柱状图。数据来自一次 git_commit_stats 命令（四套桶），
  // tab 切换零 IPC；仓库变更（repo://changed）时若开着则静默刷新。
  import * as Dialog from "$lib/components/ui/dialog";
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
  import { EmptyState } from "$lib/components/ui/empty-state";
  import { t } from "$lib/i18n";
  import {
    git,
    normalizeError,
    type CommitStatsDto,
  } from "$lib/git";
  import { onRepoChanged } from "$lib/git/events";
  import { repos } from "$lib/stores/repos.svelte";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import Check from "@lucide/svelte/icons/check";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import StatsBarChart from "./StatsBarChart.svelte";

  let {
    open = $bindable(false),
    repoId,
  }: { open?: boolean; repoId: string } = $props();

  type Tab = "overview" | "month" | "week" | "today";
  const TABS: { id: Tab; key: string }[] = [
    { id: "overview", key: "stats.tab.overview" },
    { id: "month", key: "stats.tab.month" },
    { id: "week", key: "stats.tab.week" },
    { id: "today", key: "stats.tab.today" },
  ];

  let tab = $state<Tab>("overview");
  let data = $state<CommitStatsDto | null>(null);
  let loading = $state(false);
  /** 统计目标 revision（分支全名或 HEAD）。 */
  let rev = $state("");

  const currentBranch = $derived(repos.active?.branch || "HEAD");
  const branches = $derived(repos.active?.branches ?? []);

  const buckets = $derived(
    !data
      ? []
      : tab === "overview"
        ? data.months
        : tab === "month"
          ? data.month_days
          : tab === "week"
            ? data.week_days
            : data.today_hours,
  );

  /** 当前 tab 对应周期的合计（贡献者表 + 总数），与图表桶轴同口径。 */
  const period = $derived(
    !data
      ? null
      : tab === "overview"
        ? data.all
        : tab === "month"
          ? data.month
          : tab === "week"
            ? data.week
            : data.today,
  );
  const contributors = $derived(period?.contributors ?? []);

  /** 规范桶键 → 展示标签：月 `2026-08`→`2026/08`，日取 `MM/DD`，小时加 `:00`。 */
  function formatBucketLabel(key: string): string {
    if (tab === "overview") return key.replace("-", "/");
    if (tab === "today") return `${key}:00`;
    return key.slice(5).replace("-", "/");
  }

  async function load(): Promise<void> {
    if (!repoId || !rev) return;
    loading = true;
    try {
      data = await git.commitStats(repoId, rev);
    } catch (e) {
      data = null;
      normalizeError(e);
    } finally {
      loading = false;
    }
  }

  // 切仓库 → 全部重置（下次打开按新仓库初始化）。
  $effect(() => {
    void repoId;
    rev = "";
    data = null;
    tab = "overview";
  });

  // 打开时初始化分支（当前分支，unborn/detached 回落 HEAD）。
  $effect(() => {
    if (open && !rev) rev = currentBranch;
  });

  // open/rev 变化即加载（切 tab 不重新拉取——四套桶已一次取回）。
  $effect(() => {
    if (open && rev) void load();
  });

  // 对话框开着期间仓库变更 → 静默刷新（后端已防抖）。
  $effect(() => {
    if (!open || !repoId) return;
    const p = onRepoChanged((e) => {
      if (e.repoId === repoId) void load();
    });
    return () => void p.then((un) => un());
  });
</script>

<Dialog.Root bind:open>
  <!-- 宽度必须带 sm: 前缀才能覆盖默认的 sm:max-w-sm（tailwind-merge 按
       变体分组去重）；无前缀的 max-w-3xl 会被小屏规则压掉。 -->
  <Dialog.Content class="max-w-[calc(100%-2rem)] sm:max-w-3xl">
    <Dialog.Header>
      <Dialog.Title>{t("stats.title")}</Dialog.Title>
    </Dialog.Header>

    <!-- 粒度 tab（分段样式） -->
    <div class="flex justify-center">
      <div class="flex rounded-full bg-muted p-1">
        {#each TABS as tt (tt.id)}
          <button
            type="button"
            class={`rounded-full px-4 py-1 text-xs transition-colors ${
              tab === tt.id
                ? "bg-background font-medium shadow-sm"
                : "text-muted-foreground hover:text-foreground"
            }`}
            onclick={() => (tab = tt.id)}
          >
            {t(tt.key)}
          </button>
        {/each}
      </div>
    </div>

    <div class="flex h-80 min-w-0 gap-4 pt-1">
      <!-- 左：分支 + 贡献者 -->
      <div class="flex w-52 shrink-0 flex-col gap-2">
        <DropdownMenu.Root>
          <DropdownMenu.Trigger>
            {#snippet child({ props })}
              <button
                {...props}
                type="button"
                class="flex min-h-8 w-full items-center gap-2 rounded-md border bg-background px-2.5 py-1.5 text-sm hover:bg-accent"
              >
                <GitBranch class="size-3.5 shrink-0 text-muted-foreground" />
                <span class="min-w-0 flex-1 truncate text-left">
                  {rev || currentBranch}
                </span>
                <ChevronDown class="size-3.5 shrink-0 text-muted-foreground" />
              </button>
            {/snippet}
          </DropdownMenu.Trigger>
          <DropdownMenu.Content class="max-h-72 w-56 overflow-y-auto">
            <DropdownMenu.Item
              onSelect={() => (rev = "HEAD")}
              class="gap-2 text-muted-foreground"
            >
              <GitBranch class="size-3.5 shrink-0" />
              HEAD
            </DropdownMenu.Item>
            {#each branches as b (b.full_name)}
              <DropdownMenu.Item
                onSelect={() => (rev = b.full_name)}
                class="gap-2"
              >
                <GitBranch class="size-3.5 shrink-0 text-muted-foreground" />
                <span class="min-w-0 flex-1 truncate">{b.name}</span>
                {#if b.full_name === rev}
                  <Check class="size-3.5 shrink-0" />
                {/if}
              </DropdownMenu.Item>
            {/each}
          </DropdownMenu.Content>
        </DropdownMenu.Root>

        <div class="min-h-0 flex-1 overflow-y-auto rounded-md border">
          {#each contributors as c (c.email)}
            <div
              class="flex items-center justify-between gap-2 px-2.5 py-1.5 text-xs"
            >
              <span class="min-w-0 truncate" title={c.email}>
                {c.name || c.email}
              </span>
              <span class="shrink-0 tabular-nums text-muted-foreground">
                {c.count}
              </span>
            </div>
          {:else}
            <div class="px-2.5 py-3 text-center text-xs text-muted-foreground">
              {t("stats.empty")}
            </div>
          {/each}
        </div>
      </div>

      <!-- 右：柱状图（overflow-hidden 断开 svg 固定宽度的 min-content
           链，否则会把 grid 轨道顶宽、内容溢出对话框背景盒） -->
      <div class="relative flex min-w-0 flex-1 flex-col overflow-hidden">
        {#if loading && !data}
          <div class="flex flex-1 items-center justify-center">
            <LoaderCircle class="size-5 animate-spin text-muted-foreground" />
          </div>
        {:else if !data || data.all.total === 0}
          <EmptyState
            icon={GitBranch}
            title={t("stats.empty")}
            hint={t("stats.emptyHint")}
          />
        {:else}
          <StatsBarChart buckets={buckets} formatLabel={formatBucketLabel} />
          {#if loading}
            <LoaderCircle
              class="absolute top-3 right-3 size-3.5 animate-spin text-muted-foreground"
            />
          {/if}
        {/if}
      </div>
    </div>

    <!-- 底部合计 -->
    <div
      class="flex items-center justify-between border-t pt-2 text-xs text-muted-foreground"
    >
      <span>{t("stats.contributorsN", { n: contributors.length })}</span>
      <span>{t("stats.commitsN", { n: period?.total ?? 0 })}</span>
    </div>
  </Dialog.Content>
</Dialog.Root>
