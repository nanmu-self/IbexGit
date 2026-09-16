<script lang="ts">
  // P5 提交历史主视图: virtualized commit list with the SVG graph,
  // search/filters, multi-select history operations (cherry-pick / squash /
  // revert with preview dialogs), and the commit detail panel.
  import { Input } from "$lib/components/ui/input";
  import { Button } from "$lib/components/ui/button";
  import { ConfirmDialog } from "$lib/components/ui/confirm-dialog";
  import * as Dialog from "$lib/components/ui/dialog";
  import { VirtualList } from "$lib/components/ui/virtual-list";
  import { PanelResizer } from "$lib/components/ui/panel-resizer";
  import { EmptyState } from "$lib/components/ui/empty-state";
  import GraphLane from "./GraphLane.svelte";
  import RefBadges from "./RefBadges.svelte";
  import CommitDetailPanel from "./CommitDetailPanel.svelte";
  import { t } from "$lib/i18n";
  import {
    git,
    normalizeError,
    type GraphFilter,
    type GraphRow,
  } from "$lib/git";
  import { repos } from "$lib/stores/repos.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { showToast } from "$lib/stores/toast";
  import Search from "@lucide/svelte/icons/search";
  import SlidersHorizontal from "@lucide/svelte/icons/sliders-horizontal";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  const ROW_H = 28;

  // ---- graph state ----
  let rows = $state<GraphRow[]>([]);
  let graphWidth = $state(0);
  let complete = $state(false);
  let loading = $state(false);
  let loadingMore = $state(false);

  // ---- selection ----
  /** Single selection → detail panel. */
  let selectedHash = $state<string | null>(null);
  /** Multi-selection for history operations (hashes, topo order). */
  let multi = $state<string[]>([]);
  let anchor = $state<string | null>(null);

  // ---- detail ----
  let detailOpen = $state(false);
  let detailResizing = $state(false);

  // ---- filters ----
  let searchText = $state("");
  let authorText = $state("");
  let sinceText = $state("");
  let untilText = $state("");
  let pathText = $state("");
  let filtersOpen = $state(false);

  const active = $derived(repos.active);

  const filter = $derived.by<GraphFilter>(() => {
    const f: GraphFilter = { text: null, author: null, since: null, until: null, paths: [] };
    const text = searchText.trim();
    if (text) f.text = text;
    const author = authorText.trim();
    if (author) f.author = author;
    const since = sinceText.trim();
    if (since) f.since = since;
    const until = untilText.trim();
    if (until) f.until = until;
    const path = pathText.trim();
    if (path) f.paths = [path];
    return f;
  });

  // ---- graph loading: repo switch, watcher refresh, filter change ----
  let reloadSeq = 0;
  $effect(() => {
    const id = active?.id;
    const gen = active?.lastRefreshMs;
    const f = filter;
    // Debounce filter typing; watcher-triggered reloads also pass through
    // here (gen changes after the backend re-read settled).
    const timer = setTimeout(() => {
      void loadGraph(id, f);
    }, 250);
    return () => clearTimeout(timer);
  });

  async function loadGraph(id: string | undefined, f: GraphFilter): Promise<void> {
    if (!id) {
      rows = [];
      return;
    }
    const seq = ++reloadSeq;
    loading = rows.length === 0;
    try {
      const page = await git.graph(id, f);
      if (seq !== reloadSeq || repos.activeId !== id) return;
      rows = page.rows;
      graphWidth = page.width;
      complete = page.complete;
      // Drop selections pointing at commits no longer listed.
      const present = new Set(rows.map((r) => r.commit.hash));
      multi = multi.filter((h) => present.has(h));
      if (selectedHash && !present.has(selectedHash)) {
        selectedHash = null;
        detailOpen = false;
      }
    } catch (e) {
      if (seq === reloadSeq) normalizeError(e);
    } finally {
      if (seq === reloadSeq) loading = false;
    }
  }

  async function loadMore(): Promise<void> {
    const id = repos.activeId;
    if (!id || complete || loadingMore || loading) return;
    loadingMore = true;
    try {
      const page = await git.graphMore(id, filter);
      if (page.start === 0) {
        rows = page.rows; // backend rebuilt the cache
      } else {
        rows = [...rows, ...page.rows];
      }
      graphWidth = page.width;
      complete = page.complete;
    } catch (e) {
      normalizeError(e);
    } finally {
      loadingMore = false;
    }
  }

  // ---- row interactions ----
  function plainClick(row: GraphRow): void {
    selectedHash = row.commit.hash;
    detailOpen = true;
  }

  function rowClick(row: GraphRow, e: MouseEvent): void {
    if (e.shiftKey && anchor) {
      const a = rows.findIndex((r) => r.commit.hash === anchor);
      const b = rows.findIndex((r) => r.commit.hash === row.commit.hash);
      if (a !== -1 && b !== -1) {
        const [lo, hi] = a < b ? [a, b] : [b, a];
        multi = rows.slice(lo, hi + 1).map((r) => r.commit.hash);
        return;
      }
    }
    if (e.ctrlKey || e.metaKey) {
      if (multi.includes(row.commit.hash)) {
        multi = multi.filter((h) => h !== row.commit.hash);
      } else {
        multi = [...multi, row.commit.hash];
      }
      anchor = row.commit.hash;
      return;
    }
    multi = [];
    anchor = row.commit.hash;
    plainClick(row);
  }

  // ---- multi-select operations ----
  const squashEligible = $derived.by(() => {
    const k = multi.length;
    if (k < 2 || rows.length < k) return false;
    const sel = new Set(multi);
    for (let i = 0; i < k; i++) {
      if (!sel.has(rows[i].commit.hash)) return false;
      // Merge commits cannot be squashed (v1).
      if (rows[i].commit.parents.length !== 1) return false;
    }
    for (let i = 0; i < k - 1; i++) {
      if (rows[i].commit.parents[0] !== rows[i + 1].commit.hash) return false;
    }
    return true;
  });

  let dialogMode = $state<"cherry-pick" | "revert" | "squash" | null>(null);
  let squashMessage = $state("");
  let opBusy = $state(false);

  function openDialog(mode: "cherry-pick" | "revert" | "squash"): void {
    if (mode === "squash" && !squashEligible) return;
    dialogMode = mode;
    if (mode === "squash") {
      // Default message: newest subject, remaining subjects as the body.
      const [head, ...rest] = [...multi].reverse().map(
        (h) => rows.find((r) => r.commit.hash === h)?.commit.message ?? h,
      );
      squashMessage = rest.length > 0 ? `${head}\n\n${rest.join("\n")}` : head;
    }
  }

  function closeDialog(): void {
    dialogMode = null;
  }

  async function runOp(): Promise<void> {
    const id = repos.activeId;
    if (!id || !dialogMode) return;
    opBusy = true;
    try {
      if (dialogMode === "cherry-pick") {
        await git.cherryPick(id, [...multi].reverse()); // oldest first
        showToast("success", t("history.cherryPickDone", { n: multi.length }));
      } else if (dialogMode === "revert") {
        await git.revert(id, [...multi]); // newest first
        showToast("success", t("history.revertDone", { n: multi.length }));
      } else {
        await git.squash(id, [...multi], squashMessage);
        showToast("success", t("history.squashDone", { n: multi.length }));
      }
      dialogMode = null;
      multi = [];
      await repos.refresh(id);
    } catch (e) {
      normalizeError(e);
    } finally {
      opBusy = false;
    }
  }

  const dialogCommits = $derived.by<GraphRow[]>(() => {
    if (!dialogMode) return [];
    return multi
      .map((h) => rows.find((r) => r.commit.hash === h))
      .filter((r): r is GraphRow => r !== undefined);
  });

  const dialogTitle = $derived(
    dialogMode === "cherry-pick"
      ? t("history.cherryPickTitle", { n: multi.length })
      : dialogMode === "revert"
        ? t("history.revertTitle", { n: multi.length })
        : t("history.squashTitle", { n: multi.length }),
  );

  // ---- detached HEAD guidance ----
  let createBranchOpen = $state(false);
  let newBranchName = $state("");
  let branchBusy = $state(false);

  async function createBranchHere(): Promise<void> {
    const id = repos.activeId;
    const name = newBranchName.trim();
    if (!id || !name) return;
    branchBusy = true;
    try {
      await git.createBranch(id, name);
      await git.checkoutBranch(id, name);
      createBranchOpen = false;
      newBranchName = "";
      showToast("success", t("sidebar.checkoutDone", { name }));
      await repos.refresh(id);
    } catch (e) {
      normalizeError(e);
    } finally {
      branchBusy = false;
    }
  }

  // ---- misc ----
  function fmtDate(iso: string): string {
    const d = new Date(iso);
    const now = new Date();
    const sameDay = d.toDateString() === now.toDateString();
    return sameDay
      ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
      : d.toLocaleDateString([], { month: "short", day: "numeric" });
  }

  const listKey = $derived((r: GraphRow) => r.commit.hash);
</script>

{#if !active}
  <div class="flex-1"></div>
{:else}
  <div class="relative flex min-h-0 flex-1 flex-col">
    <!-- toolbar -->
    <div class="flex items-center gap-2 border-b p-2">
      <div class="relative min-w-0 flex-1 max-w-sm">
        <Search class="absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          class="h-8 bg-background pl-7 text-[13px]"
          placeholder={t("history.search")}
          bind:value={searchText}
        />
      </div>
      <Button
        variant="ghost"
        size="icon-sm"
        title={t("history.filters")}
        onclick={() => (filtersOpen = !filtersOpen)}
      >
        <SlidersHorizontal class="size-4" />
      </Button>
      {#if loading}
        <LoaderCircle class="size-4 animate-spin text-muted-foreground" />
      {/if}
      <span class="ml-auto shrink-0 text-xs text-muted-foreground">
        {t("history.loaded", { n: rows.length })}
        {#if !complete}
          · {t("history.moreHint")}
        {/if}
      </span>
    </div>

    {#if filtersOpen}
      <div class="flex flex-wrap items-center gap-2 border-b bg-muted/20 p-2">
        <Input
          class="h-7 w-40 bg-background text-xs"
          placeholder={t("history.filterAuthor")}
          bind:value={authorText}
        />
        <Input
          class="h-7 w-32 bg-background text-xs"
          type="date"
          placeholder={t("history.filterSince")}
          bind:value={sinceText}
        />
        <Input
          class="h-7 w-32 bg-background text-xs"
          type="date"
          placeholder={t("history.filterUntil")}
          bind:value={untilText}
        />
        <Input
          class="h-7 w-44 bg-background text-xs"
          placeholder={t("history.filterPath")}
          bind:value={pathText}
        />
        <Button
          variant="ghost"
          size="xs"
          onclick={() => {
            authorText = "";
            sinceText = "";
            untilText = "";
            pathText = "";
          }}
        >
          {t("history.clearFilters")}
        </Button>
      </div>
    {/if}

    <!-- detached HEAD banner -->
    {#if active.detached}
      <div
        class="flex items-center gap-2 border-b bg-amber-500/10 px-3 py-1.5 text-xs text-amber-700 dark:text-amber-400"
      >
        <span class="min-w-0 flex-1 truncate">{t("history.detachedBanner")}</span>
        <Button variant="outline" size="xs" onclick={() => (createBranchOpen = true)}>
          <GitBranch class="size-3" />
          {t("history.createBranchHere")}
        </Button>
      </div>
    {/if}

    <!-- list + detail -->
    <div class="flex min-h-0 flex-1">
      <!-- flex-col so VirtualList's min-h-0 flex-1 actually constrains its height -->
      <div class="flex min-h-0 min-w-0 flex-1 flex-col">
        {#if rows.length === 0 && !loading}
          <EmptyState icon={GitBranch} title={t("history.empty")} hint={t("history.emptyHint")} />
        {:else}
          <VirtualList
            items={rows}
            itemHeight={ROW_H}
            overscan={10}
            getKey={listKey}
            onNearBottom={() => void loadMore()}
          >
            {#snippet row(row: GraphRow, index: number)}
              <div
                class="flex cursor-default items-center {selectedHash === row.commit.hash ||
                multi.includes(row.commit.hash)
                  ? 'bg-accent/60'
                  : index % 2 === 1
                    ? 'bg-muted/20'
                    : ''} hover:bg-muted/50"
                style="height: {ROW_H}px"
                onclick={(e) => rowClick(row, e)}
                role="button"
                tabindex="0"
                onkeydown={(e) => e.key === "Enter" && plainClick(row)}
              >
                <GraphLane
                  row={row}
                  lanes={Math.min(Math.max(graphWidth, row.lane + 1), 16)}
                  rowHeight={ROW_H}
                  isHead={row.commit.refs.some((r) => r.startsWith("HEAD"))}
                />
                <div class="flex min-w-0 flex-1 items-center gap-2 pr-3">
                  <RefBadges refs={row.commit.refs} />
                  <span class="min-w-0 flex-1 truncate text-[13px]">
                    {row.commit.message}
                  </span>
                  <span class="shrink-0 text-xs text-muted-foreground">
                    {row.commit.author}
                  </span>
                  <span class="w-16 shrink-0 text-right text-xs text-muted-foreground">
                    {fmtDate(row.commit.date)}
                  </span>
                </div>
              </div>
            {/snippet}
          </VirtualList>
        {/if}
      </div>

      {#if detailOpen && selectedHash}
        <!-- direct flex child: stretches to full row height (a wrapper div
             would collapse the 4px grip to height 0 and make it undraggable) -->
        <!-- sized panel sits right of the grip → side="right" keeps the
             grip tracking the cursor (dragging right narrows the detail) -->
        <PanelResizer
          bind:width={settings.historyDetailWidth}
          side="right"
          min={280}
          max={720}
          bind:dragging={detailResizing}
          onCommit={(w) => void settings.setHistoryDetailWidth(w)}
        />
        <CommitDetailPanel hash={selectedHash} onClose={() => (detailOpen = false)} />
      {/if}
    </div>

    <!-- multi-select action bar -->
    {#if multi.length > 0}
      <div
        class="absolute bottom-3 left-1/2 z-20 flex -translate-x-1/2 items-center gap-2 rounded-lg border bg-popover p-1.5 shadow-lg"
      >
        <span class="px-1 text-xs text-muted-foreground">
          {t("history.selected", { n: multi.length })}
        </span>
        <Button variant="outline" size="xs" onclick={() => openDialog("cherry-pick")}>
          {t("history.cherryPick")}
        </Button>
        <Button
          variant="outline"
          size="xs"
          disabled={!squashEligible}
          title={squashEligible ? "" : t("history.squashNotEligible")}
          onclick={() => openDialog("squash")}
        >
          {t("history.squash")}
        </Button>
        <Button variant="outline" size="xs" onclick={() => openDialog("revert")}>
          {t("history.revert")}
        </Button>
        <Button variant="ghost" size="icon-sm" onclick={() => (multi = [])}>
          ✕
        </Button>
      </div>
    {/if}
  </div>

  <!-- operation preview dialog -->
  <Dialog.Root open={dialogMode !== null} onOpenChange={(o) => !o && closeDialog()}>
    <Dialog.Content class="max-w-md">
      <Dialog.Header>
        <Dialog.Title>{dialogTitle}</Dialog.Title>
        <Dialog.Description>
          {dialogMode === "squash"
            ? t("history.squashDesc")
            : t("history.opPreviewDesc")}
        </Dialog.Description>
      </Dialog.Header>
      <div class="space-y-2">
        {#if dialogMode === "squash"}
          <textarea
            class="min-h-24 w-full resize-y rounded-md border bg-background p-2 text-sm"
            bind:value={squashMessage}
          ></textarea>
        {/if}
        <div class="max-h-40 overflow-y-auto rounded-md border p-2 text-xs">
          {#each dialogCommits as row (row.commit.hash)}
            <div class="flex items-baseline gap-2">
              <span class="shrink-0 font-mono text-muted-foreground">
                {row.commit.short_hash}
              </span>
              <span class="min-w-0 flex-1 truncate">{row.commit.message}</span>
            </div>
          {/each}
        </div>
        {#if dialogMode === "squash"}
          <p class="text-xs text-muted-foreground">{t("history.squashWarn")}</p>
        {/if}
      </div>
      <Dialog.Footer>
        <Button variant="outline" onclick={closeDialog}>{t("common.cancel")}</Button>
        <Button disabled={opBusy} onclick={() => void runOp()}>{t("common.ok")}</Button>
      </Dialog.Footer>
    </Dialog.Content>
  </Dialog.Root>

  <!-- create branch on detached HEAD -->
  <Dialog.Root bind:open={createBranchOpen}>
    <Dialog.Content class="max-w-sm">
      <Dialog.Header>
        <Dialog.Title>{t("history.createBranchTitle")}</Dialog.Title>
        <Dialog.Description>{t("history.createBranchDesc")}</Dialog.Description>
      </Dialog.Header>
      <Input placeholder={t("history.branchName")} bind:value={newBranchName} />
      <Dialog.Footer>
        <Button variant="outline" onclick={() => (createBranchOpen = false)}>
          {t("common.cancel")}
        </Button>
        <Button disabled={branchBusy || !newBranchName.trim()} onclick={() => void createBranchHere()}>
          {t("common.ok")}
        </Button>
      </Dialog.Footer>
    </Dialog.Content>
  </Dialog.Root>
{/if}
