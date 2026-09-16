<script lang="ts">
  // P9 单文件历史：`log --follow` 提交列表（跨重命名连续追溯）+ 选中提交
  // 的该文件 diff 预览（复用 DiffViewer，source=commit 只读）。
  // 分页为游标式（start = 本页末尾 hash），rename 边界不断链。
  import { Button } from "$lib/components/ui/button";
  import { VirtualList } from "$lib/components/ui/virtual-list";
  import { PanelResizer } from "$lib/components/ui/panel-resizer";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { t } from "$lib/i18n";
  import {
    git,
    normalizeError,
    EMPTY_TREE_SHA,
    type FileCommit,
    type DiffModel,
  } from "$lib/git";
  import { repos } from "$lib/stores/repos.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import GitCommitHorizontal from "@lucide/svelte/icons/git-commit-horizontal";

  const ROW_H = 42;
  const PAGE = 500;

  let { path, jumpHash }: { path: string; jumpHash: string | null } = $props();

  let entries = $state<FileCommit[]>([]);
  let loading = $state(false);
  let loadingMore = $state(false);
  let complete = $state(false);
  let selectedHash = $state<string | null>(null);
  /** Blame → diff jump target, consumed once applied. The dialog remounts
   *  this list on every open/tab-switch, so capturing the initial prop is
   *  intentional — later store changes must not override user selection. */
  // svelte-ignore state_referenced_locally
  let pendingJump = $state<string | null>(jumpHash);

  // ---- diff state (context expansion + ignore whitespace, like P4) ----
  let diffModel = $state<DiffModel | null>(null);
  let diffLoading = $state(false);
  let diffIgnoreWs = $state(false);
  let diffContext = $state<{ hash: string | null; context: number }>({
    hash: null,
    context: 3,
  });

  const selected = $derived(entries.find((e) => e.hash === selectedHash) ?? null);
  const active = $derived(repos.active);

  let list = $state<{
    scrollToIndex: (i: number) => void;
  } | null>(null);

  // ---- load first page whenever (repo, path) changes ----
  let reloadSeq = 0;
  $effect(() => {
    const id = repos.activeId;
    const p = path;
    const seq = ++reloadSeq;
    entries = [];
    selectedHash = null;
    diffModel = null;
    complete = false;
    if (!id || !p) return;
    loading = true;
    git
      .fileHistory(id, p, PAGE)
      .then((page) => {
        if (seq !== reloadSeq) return;
        entries = page;
        complete = page.length < PAGE;
      })
      .catch((e) => {
        if (seq === reloadSeq) normalizeError(e);
      })
      .finally(() => {
        if (seq === reloadSeq) loading = false;
      });
  });

  async function loadMore(): Promise<void> {
    const id = repos.activeId;
    const last = entries[entries.length - 1];
    if (!id || !last || complete || loading || loadingMore) return;
    loadingMore = true;
    try {
      const page = await git.fileHistory(id, path, PAGE, last.hash);
      entries = [...entries, ...page];
      complete = page.length < PAGE;
    } catch (e) {
      normalizeError(e);
    } finally {
      loadingMore = false;
    }
  }

  // ---- blame → history jump: select the target commit and center it;
  // keep paging until it is found, then consume the pending target so
  // later loads don't fight the user's selection ----
  $effect(() => {
    const target = pendingJump;
    if (!target) return;
    const idx = entries.findIndex((e) => e.hash === target);
    if (idx >= 0) {
      select(entries[idx], idx);
      pendingJump = null;
    } else if (!complete && !loading && !loadingMore) {
      void loadMore();
    }
  });

  function select(entry: FileCommit, index: number): void {
    selectedHash = entry.hash;
    diffContext = { hash: entry.hash, context: 3 };
    list?.scrollToIndex(index);
  }

  // ---- diff loading for the selected commit ----
  $effect(() => {
    const id = repos.activeId;
    const entry = selected;
    const refreshMark = active?.lastRefreshMs;
    if (!id || !entry) {
      diffModel = null;
      diffLoading = false;
      return;
    }
    // Effective context: the override only applies to the commit it was
    // expanded for; switching commits falls back to git's default 3.
    const context = diffContext.hash === entry.hash ? diffContext.context : 3;
    diffLoading = true;
    const paths = entry.orig_path ? [entry.path, entry.orig_path] : [entry.path];
    const oldRev = entry.parents[0] ?? EMPTY_TREE_SHA;
    git
      .diff(id, "commit", oldRev, entry.hash, paths, context, diffIgnoreWs)
      .then((model) => {
        if (selectedHash === entry.hash) diffModel = model;
      })
      .catch((e) => {
        normalizeError(e);
        diffModel = null;
      })
      .finally(() => {
        if (selectedHash === entry.hash) diffLoading = false;
      });
    void refreshMark;
  });

  function handleExpand(_path: string, dir: "up" | "down" | "all"): void {
    const entry = selected;
    if (!entry) return;
    const current = diffContext.hash === entry.hash ? diffContext.context : 3;
    const context = dir === "all" ? 100_000 : Math.min(current + 10, 100_000);
    diffContext = { hash: entry.hash, context };
  }

  function handleIgnoreWs(v: boolean): void {
    diffIgnoreWs = v;
  }

  // ---- misc ----
  function fmtDate(iso: string): string {
    const d = new Date(iso);
    return d.toLocaleDateString([], { year: "numeric", month: "short", day: "numeric" });
  }

  function statusColor(status: string): string {
    switch (status) {
      case "A":
        return "text-emerald-600 dark:text-emerald-400";
      case "D":
        return "text-red-600 dark:text-red-400";
      case "R":
      case "C":
        return "text-violet-600 dark:text-violet-400";
      default:
        return "text-amber-600 dark:text-amber-400";
    }
  }
</script>

<div class="flex min-h-0 flex-1">
  <!-- commit list -->
  <div
    class="flex min-h-0 flex-col border-r"
    style="width: {settings.fileHistoryListWidth}px"
  >
    {#if loading && entries.length === 0}
      <div class="p-4 text-sm text-muted-foreground">{t("common.loading")}</div>
    {:else if entries.length === 0}
      <div class="flex flex-1 items-center justify-center p-4 text-center text-sm text-muted-foreground">
        <div>
          <GitCommitHorizontal class="mx-auto mb-2 size-6 opacity-40" />
          {t("fileView.empty")}
        </div>
      </div>
    {:else}
      <VirtualList
        bind:this={list}
        items={entries}
        itemHeight={ROW_H}
        overscan={10}
        getKey={(e) => e.hash}
        onNearBottom={() => void loadMore()}
      >
        {#snippet row(entry: FileCommit, index: number)}
          <div
            class="flex cursor-default flex-col justify-center gap-0.5 px-3 {selectedHash === entry.hash
              ? 'bg-accent/60'
              : index % 2 === 1
                ? 'bg-muted/20'
                : ''} hover:bg-muted/50"
            style="height: {ROW_H}px"
            onclick={() => select(entry, index)}
            role="button"
            tabindex="0"
            onkeydown={(e) => e.key === "Enter" && select(entry, index)}
          >
            <div class="flex min-w-0 items-center gap-2">
              <span class="w-3 shrink-0 text-center font-mono text-[11px] font-bold {statusColor(entry.status)}">
                {entry.status}
              </span>
              <span class="min-w-0 flex-1 truncate text-[13px]" title={entry.message}>
                {entry.message}
              </span>
            </div>
            <div class="flex min-w-0 items-center gap-2 text-[11px] text-muted-foreground">
              <span class="shrink-0 font-mono">{entry.short_hash}</span>
              <span class="min-w-0 flex-1 truncate">{entry.author}</span>
              <span class="shrink-0">{fmtDate(entry.date)}</span>
            </div>
          </div>
        {/snippet}
      </VirtualList>
    {/if}
    <div class="flex items-center gap-2 border-t px-3 py-1 text-[11px] text-muted-foreground">
      <span class="truncate">{t("fileView.commitsN", { n: entries.length })}</span>
      {#if !complete}
        <Button variant="ghost" size="xs" class="ml-auto" disabled={loadingMore} onclick={() => void loadMore()}>
          {t("fileView.loadMore")}
        </Button>
      {/if}
    </div>
  </div>

  <PanelResizer
    bind:width={settings.fileHistoryListWidth}
    min={260}
    max={560}
    onCommit={(w) => void settings.setFileHistoryListWidth(w)}
  />

  <!-- diff preview -->
  <div class="min-w-0 flex-1">
    {#if selected}
      <DiffViewer
        model={diffModel}
        loading={diffLoading}
        repoId={repos.activeId}
        ignoreWhitespace={diffIgnoreWs}
        onlineop={() => {}}
        onexpand={handleExpand}
        onignorewschange={handleIgnoreWs}
      />
    {:else}
      <div class="flex h-full items-center justify-center p-4 text-center text-sm text-muted-foreground">
        {t("fileView.selectCommit")}
      </div>
    {/if}
  </div>
</div>
