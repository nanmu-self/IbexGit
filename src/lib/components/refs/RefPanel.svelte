<script lang="ts">
  /**
   * RefPanel (P6) — sidebar reference browser: branches, remotes, tags,
   * stashes and the HEAD reflog. Rows carry context menus; dialogs and
   * destructive flows are delegated to RefsDialogsHost via the ref bus.
   */
  import { fade } from "svelte/transition";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { refsData, loadRefsData, REFS_RELOG_LIMIT } from "$lib/stores/refsdata.svelte";
  import { requestRefAction } from "$lib/stores/refbus";
  import { git, normalizeError, type BranchInfo, type ReflogEntry, type RemoteInfo, type StashEntry, type TagInfo } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import ChevronsUpDown from "@lucide/svelte/icons/chevrons-up-down";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import GitBranchPlus from "@lucide/svelte/icons/git-branch-plus";
  import GitCompare from "@lucide/svelte/icons/git-compare";
  import GitFork from "@lucide/svelte/icons/git-fork";
  import RotateCcw from "@lucide/svelte/icons/rotate-ccw";
  import Tag from "@lucide/svelte/icons/tag";
  import Archive from "@lucide/svelte/icons/archive";
  import Plus from "@lucide/svelte/icons/plus";
  import MoreHorizontal from "@lucide/svelte/icons/more-horizontal";
  import Scissors from "@lucide/svelte/icons/scissors";
  import ArrowDown from "@lucide/svelte/icons/arrow-down";
  import ArrowUp from "@lucide/svelte/icons/arrow-up";
  import CircleDot from "@lucide/svelte/icons/circle-dot";

  // ---- section collapse (persisted via the per-repo UI state) ----
  const SECTIONS = ["branches", "remote", "tags", "stash", "reflog"] as const;
  type SectionId = (typeof SECTIONS)[number];

  const expanded = $derived.by(() => {
    const collapsed = new Set(repos.ui.sidebar_collapsed);
    return new Set(SECTIONS.filter((id) => !collapsed.has(id)));
  });

  function toggleSection(id: SectionId): void {
    const collapsed = new Set(repos.ui.sidebar_collapsed);
    if (expanded.has(id)) collapsed.add(id);
    else collapsed.delete(id);
    repos.updateUi({ sidebar_collapsed: [...collapsed] });
  }

  // ---- branch sorting: name | ahead/behind (PLAN P6) ----
  let sortByActivity = $state(false);

  const branches = $derived(repos.active?.branches ?? []);
  const current = $derived(branches.find((b) => b.current) ?? null);

  const sortedBranches = $derived.by(() => {
    const rest = branches.filter((b) => !b.current);
    if (sortByActivity) {
      rest.sort(
        (a, b) => b.ahead + b.behind - (a.ahead + a.behind) || a.name.localeCompare(b.name)
      );
    } else {
      rest.sort((a, b) => a.name.localeCompare(b.name));
    }
    return current ? [current, ...rest] : rest;
  });

  // ---- data (tags/remotes/stash/reflog from the shared store) ----
  const tags = $derived(refsData.tags);
  const remotes = $derived(refsData.remotes);
  const stashes = $derived(refsData.stashes);
  const reflog = $derived(refsData.reflog);

  // Reload refs data when the repo changes or after a watcher refresh.
  let lastRefreshMark: number | null = null;
  $effect(() => {
    const id = repos.activeId;
    const mark = repos.active?.lastRefreshMs ?? null;
    if (id === null) return;
    if (mark !== lastRefreshMark) {
      lastRefreshMark = mark;
      void loadRefsData(id);
    }
  });

  // ---- context menu (single shared positioned menu) ----
  type MenuTarget =
    | { kind: "branch"; branch: BranchInfo; x: number; y: number }
    | { kind: "remote"; remote: RemoteInfo; x: number; y: number }
    | { kind: "tag"; tag: TagInfo; x: number; y: number }
    | { kind: "stash"; stash: StashEntry; x: number; y: number }
    | { kind: "reflog"; entry: ReflogEntry; x: number; y: number }
    | { kind: "panel"; x: number; y: number };

  let menu = $state<MenuTarget | null>(null);

  function openMenu(e: MouseEvent, build: (x: number, y: number) => MenuTarget): void {
    e.preventDefault();
    menu = build(e.clientX, e.clientY);
  }

  $effect(() => {
    if (!menu) return;
    const ondown = (e: MouseEvent): void => {
      const el = document.getElementById("ref-context-menu");
      if (el && !el.contains(e.target as Node)) close();
    };
    const onescape = (e: KeyboardEvent): void => {
      if (e.key === "Escape") close();
    };
    function close(): void {
      menu = null;
    }
    window.addEventListener("mousedown", ondown, true);
    window.addEventListener("keydown", onescape, true);
    return () => {
      window.removeEventListener("mousedown", ondown, true);
      window.removeEventListener("keydown", onescape, true);
    };
  });

  function menuRun(fn: () => void): () => void {
    return () => {
      fn();
      menu = null;
    };
  }

  // ---- direct actions ----
  async function checkout(name: string): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    try {
      await git.checkoutBranch(id, name);
      showToast("success", t("sidebar.checkoutDone", { name }));
      await repos.refresh(id);
      await loadRefsData(id);
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
      await loadRefsData(id);
    } catch (e) {
      normalizeError(e);
    }
  }

  async function stashApply(index: number, pop: boolean): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    try {
      if (pop) await git.stashPop(id, index);
      else await git.stashApply(id, index);
      showToast("success", t(pop ? "refs.stash.popped" : "refs.stash.applied"));
      await repos.refresh(id);
      await loadRefsData(id);
    } catch (e) {
      normalizeError(e);
    }
  }

  async function fetchRemote(name: string): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    try {
      await git.fetch(id, name);
      showToast("success", t("netops.fetchDone"));
      await repos.refresh(id);
      await loadRefsData(id);
    } catch (e) {
      normalizeError(e);
    }
  }

  async function pruneRemote(name: string): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    try {
      await git.pruneRemote(id, name);
      showToast("success", t("refs.remote.pruned", { name }));
      await repos.refresh(id);
      await loadRefsData(id);
    } catch (e) {
      normalizeError(e);
    }
  }

  const hasRepo = $derived(repos.activeId !== null);
</script>

<div class="flex flex-col gap-0.5">
  <!-- ============ panel actions (overflow) ============ -->
  <div class="flex items-center justify-between px-2 pt-2 pb-1">
    <span class="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
      {t("refs.panel.title")}
    </span>
    <button
      type="button"
      class="rounded p-0.5 hover:bg-accent disabled:opacity-40"
      disabled={!hasRepo}
      title={t("refs.panel.more")}
      onclick={(e) => openMenu(e, (x, y) => ({ kind: "panel", x, y }))}
    >
      <MoreHorizontal class="size-4" />
    </button>
  </div>

  {#if !hasRepo}
    <p class="px-2 pb-2 text-xs text-muted-foreground">{t("statusbar.noRepo")}</p>
  {/if}

  <!-- ============ branches ============ -->
  <div>
    <div
      role="button"
      tabindex="0"
      class="flex w-full cursor-default items-center gap-1 rounded px-2 py-1 text-xs font-medium hover:bg-accent/60"
      onclick={() => toggleSection("branches")}
      onkeydown={(e) => e.key === "Enter" && toggleSection("branches")}
    >
      <GitBranch class="size-3.5" />
      <span>{t("sidebar.branches")}</span>
      <span class="ml-auto flex items-center gap-0.5">
        <span
          role="button"
          tabindex="0"
          class="rounded p-0.5 hover:bg-accent {sortByActivity ? 'text-foreground' : 'text-muted-foreground'}"
          title={t("refs.sortActivity")}
          onclick={(e) => {
            e.stopPropagation();
            sortByActivity = !sortByActivity;
          }}
          onkeydown={(e) => e.key === "Enter" && (sortByActivity = !sortByActivity)}
        >
          <ChevronsUpDown class="size-3.5" />
        </span>
        <span
          role="button"
          tabindex="0"
          class="rounded p-0.5 text-muted-foreground hover:bg-accent"
          title={t("refs.newBranch.title")}
          onclick={(e) => {
            e.stopPropagation();
            requestRefAction({ kind: "newBranch" });
          }}
          onkeydown={(e) =>
            e.key === "Enter" && requestRefAction({ kind: "newBranch" })}
        >
          <GitBranchPlus class="size-3.5" />
        </span>
      </span>
    </div>
    {#if expanded.has("branches")}
      <ul class="pb-1">
        {#each sortedBranches as b (b.name)}
          <li class="min-w-0"><div
            class="group flex cursor-default items-center gap-1.5 rounded px-2 py-[3px] text-[13px] hover:bg-accent/70 {b.current ? 'font-medium text-foreground' : ''}"
            role="button"
            tabindex="0"
            ondblclick={() => !b.current && checkout(b.name)}
            oncontextmenu={(e) => openMenu(e, (x, y) => ({ kind: "branch", branch: b, x, y }))}
            onkeydown={(e) => e.key === "Enter" && !b.current && checkout(b.name)}
          >
            {#if b.current}
              <CircleDot class="size-3 shrink-0 text-emerald-600 dark:text-emerald-400" />
            {:else}
              <GitBranch class="size-3 shrink-0 text-muted-foreground/50" />
            {/if}
            <span class="min-w-0 flex-1 truncate" title="{b.name}{b.upstream ? ` → ${b.upstream}` : ''}">
              {b.name}
            </span>
            {#if b.upstream}
              <span class="hidden shrink-0 text-[10px] text-muted-foreground group-hover:inline">
                {b.upstream}
              </span>
            {/if}
            {#if b.ahead || b.behind}
              <span class="flex shrink-0 items-center gap-0.5 text-[10px] tabular-nums text-muted-foreground">
                {#if b.ahead}<span class="text-blue-500">↑{b.ahead}</span>{/if}
                {#if b.behind}<span class="text-red-500">↓{b.behind}</span>{/if}
              </span>
            {/if}
          </div></li>
        {:else}
          <li class="px-2 py-1 text-xs text-muted-foreground">{t("sidebar.noBranches")}</li>
        {/each}
      </ul>
    {/if}
  </div>

  <!-- ============ remotes ============ -->
  <div>
    <div
      role="button"
      tabindex="0"
      class="flex w-full cursor-default items-center gap-1 rounded px-2 py-1 text-xs font-medium hover:bg-accent/60"
      onclick={() => toggleSection("remote")}
      onkeydown={(e) => e.key === "Enter" && toggleSection("remote")}
    >
      <GitFork class="size-3.5" />
      <span>{t("sidebar.remote")}</span>
      <span class="ml-auto flex items-center gap-0.5">
        <span
          role="button"
          tabindex="0"
          class="rounded p-0.5 text-muted-foreground hover:bg-accent"
          title={t("refs.remote.addTitle")}
          onclick={(e) => {
            e.stopPropagation();
            requestRefAction({ kind: "addRemote" });
          }}
          onkeydown={(e) => e.key === "Enter" && requestRefAction({ kind: "addRemote" })}
        >
          <Plus class="size-3.5" />
        </span>
      </span>
    </div>
    {#if expanded.has("remote")}
      <ul class="pb-1">
        {#each remotes as r (r.name)}
          <li class="min-w-0"><div
            class="flex cursor-default items-center gap-1.5 rounded px-2 py-[3px] text-[13px] hover:bg-accent/70"
            role="button"
            tabindex="0"
            oncontextmenu={(e) => openMenu(e, (x, y) => ({ kind: "remote", remote: r, x, y }))}
            onkeydown={(e) => e.key === "Enter" && fetchRemote(r.name)}
          >
            <GitFork class="size-3 shrink-0 text-muted-foreground/50" />
            <span class="min-w-0 flex-1 truncate" title={r.fetch_url}>{r.name}</span>
          </div></li>
        {:else}
          <li class="px-2 py-1 text-xs text-muted-foreground">{t("refs.remote.none")}</li>
        {/each}
      </ul>
    {/if}
  </div>

  <!-- ============ tags ============ -->
  <div>
    <div
      role="button"
      tabindex="0"
      class="flex w-full cursor-default items-center gap-1 rounded px-2 py-1 text-xs font-medium hover:bg-accent/60"
      onclick={() => toggleSection("tags")}
      onkeydown={(e) => e.key === "Enter" && toggleSection("tags")}
    >
      <Tag class="size-3.5" />
      <span>{t("sidebar.tags")}</span>
      <span class="ml-auto flex items-center gap-0.5">
        <span
          role="button"
          tabindex="0"
          class="rounded p-0.5 text-muted-foreground hover:bg-accent"
          title={t("refs.tagDialog.title")}
          onclick={(e) => {
            e.stopPropagation();
            requestRefAction({ kind: "newTag" });
          }}
          onkeydown={(e) => e.key === "Enter" && requestRefAction({ kind: "newTag" })}
        >
          <Plus class="size-3.5" />
        </span>
      </span>
    </div>
    {#if expanded.has("tags")}
      <ul class="pb-1">
        {#each tags.slice(0, 50) as tag (tag.name)}
          <li class="min-w-0"><div
            class="flex cursor-default items-center gap-1.5 rounded px-2 py-[3px] text-[13px] hover:bg-accent/70"
            role="button"
            tabindex="0"
            oncontextmenu={(e) => openMenu(e, (x, y) => ({ kind: "tag", tag, x, y }))}
            onkeydown={(e) => e.key === "Enter" && checkout(tag.name)}
          >
            <Tag class="size-3 shrink-0 text-muted-foreground/50" />
            <span class="min-w-0 flex-1 truncate" title={tag.message ?? tag.name}>{tag.name}</span>
          </div></li>
        {:else}
          <li class="px-2 py-1 text-xs text-muted-foreground">{t("refs.tags.none")}</li>
        {/each}
        {#if tags.length > 50}
          <li class="px-2 py-1 text-[10px] text-muted-foreground">
            {t("refs.tags.more", { n: tags.length - 50 })}
          </li>
        {/if}
      </ul>
    {/if}
  </div>

  <!-- ============ stash ============ -->
  <div>
    <div
      role="button"
      tabindex="0"
      class="flex w-full cursor-default items-center gap-1 rounded px-2 py-1 text-xs font-medium hover:bg-accent/60"
      onclick={() => toggleSection("stash")}
      onkeydown={(e) => e.key === "Enter" && toggleSection("stash")}
    >
      <Archive class="size-3.5" />
      <span>{t("sidebar.stash")}</span>
      <span class="ml-auto flex items-center gap-0.5">
        <span
          role="button"
          tabindex="0"
          class="rounded p-0.5 text-muted-foreground hover:bg-accent"
          title={t("refs.stash.pushTitle")}
          onclick={(e) => {
            e.stopPropagation();
            void stashAll();
          }}
          onkeydown={(e) => e.key === "Enter" && void stashAll()}
        >
          <Plus class="size-3.5" />
        </span>
      </span>
    </div>
    {#if expanded.has("stash")}
      <ul class="pb-1">
        {#each stashes as s (s.index)}
          <li class="min-w-0"><div
            class="flex cursor-default items-center gap-1.5 rounded px-2 py-[3px] text-[13px] hover:bg-accent/70"
            role="button"
            tabindex="0"
            onclick={() => requestRefAction({ kind: "stashView", index: s.index })}
            oncontextmenu={(e) => openMenu(e, (x, y) => ({ kind: "stash", stash: s, x, y }))}
            onkeydown={(e) =>
              e.key === "Enter" && requestRefAction({ kind: "stashView", index: s.index })}
          >
            <Archive class="size-3 shrink-0 text-muted-foreground/50" />
            <span class="min-w-0 flex-1 truncate" title="{s.message} · {s.date}">
              <span class="text-muted-foreground">stash@{`{${s.index}}`}</span>
              {s.message}
            </span>
          </div></li>
        {:else}
          <li class="px-2 py-1 text-xs text-muted-foreground">{t("refs.stash.none")}</li>
        {/each}
      </ul>
    {/if}
  </div>

  <!-- ============ reflog ============ -->
  <div>
    <div
      role="button"
      tabindex="0"
      class="flex w-full cursor-default items-center gap-1 rounded px-2 py-1 text-xs font-medium hover:bg-accent/60"
      onclick={() => toggleSection("reflog")}
      onkeydown={(e) => e.key === "Enter" && toggleSection("reflog")}
    >
      <RotateCcw class="size-3.5" />
      <span>{t("refs.reflog.title")}</span>
    </div>
    {#if expanded.has("reflog")}
      <ul class="pb-1">
        {#each reflog.slice(0, REFS_RELOG_LIMIT) as entry, i (i)}
          <li class="min-w-0"><div
            class="flex cursor-default items-baseline gap-1.5 rounded px-2 py-[3px] text-[13px] hover:bg-accent/70"
            role="button"
            tabindex="0"
            title="{entry.short_hash} {entry.message} · {entry.date}"
            oncontextmenu={(e) => openMenu(e, (x, y) => ({ kind: "reflog", entry, x, y }))}
            onkeydown={(e) => e.key === "Enter" && checkout(entry.short_hash)}
          >
            <span class="w-9 shrink-0 font-mono text-[10px] leading-4 text-muted-foreground">
              {entry.short_hash}
            </span>
            <span class="ml-2 flex-1 truncate text-xs">{entry.message}</span>
          </div></li>
        {:else}
          <li class="px-2 py-1 text-xs text-muted-foreground">{t("refs.reflog.none")}</li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<!-- ============ shared context menu ============ -->
{#if menu}
  <div
    id="ref-context-menu"
    class="fixed z-50 min-w-48 rounded-md border bg-popover p-1 text-popover-foreground shadow-md"
    transition:fade={{ duration: 100 }}
    style="left:{Math.min(menu.x, window.innerWidth - 230)}px;top:{Math.min(
      menu.y,
      window.innerHeight - 320
    )}px"
  >
    {#if menu.kind === "branch"}
      {@const b = menu.branch}
      <button type="button" class="menu-item" disabled={b.current} onclick={menuRun(() => checkout(b.name))}>
        <CircleDot class="size-3.5" /> {t("refs.menu.checkout")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "compare", left: b.name }))}>
        <GitCompare class="size-3.5" /> {t("refs.menu.compare")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "merge", target: b.name }))}>
        <ArrowDown class="size-3.5" /> {t("refs.menu.mergeInto")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "rebase", target: b.name }))}>
        <ArrowUp class="size-3.5" /> {t("refs.menu.rebaseOnto")}
      </button>
      <div class="my-1 h-px bg-border"></div>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "newBranch", start: b.name }))}>
        <GitBranchPlus class="size-3.5" /> {t("refs.menu.branchFrom")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "newTag", target: b.name }))}>
        <Tag class="size-3.5" /> {t("refs.menu.tagHere")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "renameBranch", name: b.name }))}>
        {t("refs.menu.rename")}
      </button>
      <button
        type="button"
        class="menu-item"
        onclick={menuRun(() => requestRefAction({ kind: "setUpstream", branch: b.name, upstream: b.upstream ?? null }))}
      >
        {t("refs.menu.setUpstream")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "deleteBranch", name: b.name }))}>
        <Scissors class="size-3.5" /> {t("refs.menu.delete")}
      </button>
    {:else if menu.kind === "remote"}
      {@const r = menu.remote}
      <button type="button" class="menu-item" onclick={menuRun(() => fetchRemote(r.name))}>
        {t("refs.menu.fetch")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => pruneRemote(r.name))}>
        {t("refs.menu.prune")}
      </button>
      <div class="my-1 h-px bg-border"></div>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "editRemote", name: r.name }))}>
        {t("refs.menu.editUrl")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "removeRemote", name: r.name }))}>
        <Scissors class="size-3.5" /> {t("refs.menu.removeRemote")}
      </button>
    {:else if menu.kind === "tag"}
      {@const tag = menu.tag}
      <button type="button" class="menu-item" onclick={menuRun(() => checkout(tag.name))}>
        <CircleDot class="size-3.5" /> {t("refs.menu.checkoutDetached")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "deleteTag", name: tag.name }))}>
        <Scissors class="size-3.5" /> {t("refs.menu.delete")}
      </button>
    {:else if menu.kind === "stash"}
      {@const s = menu.stash}
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "stashView", index: s.index }))}>
        {t("refs.menu.stashDiff")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => stashApply(s.index, false))}>
        {t("refs.stash.apply")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => stashApply(s.index, true))}>
        {t("refs.stash.pop")}
      </button>
      <div class="my-1 h-px bg-border"></div>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "stashDrop", index: s.index }))}>
        <Scissors class="size-3.5" /> {t("refs.stash.drop")}
      </button>
    {:else if menu.kind === "reflog"}
      {@const entry = menu.entry}
      <button type="button" class="menu-item" onclick={menuRun(() => checkout(entry.short_hash))}>
        <CircleDot class="size-3.5" /> {t("refs.menu.checkoutDetached")}
      </button>
      <button
        type="button"
        class="menu-item"
        onclick={menuRun(() => requestRefAction({ kind: "reflogRestore", hash: entry.hash, subject: entry.message }))}
      >
        <RotateCcw class="size-3.5" /> {t("refs.reflog.restoreHere")}
      </button>
    {:else if menu.kind === "panel"}
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "reset" }))}>
        <RotateCcw class="size-3.5" /> {t("refs.menu.reset")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "clean" }))}>
        <Scissors class="size-3.5" /> {t("refs.menu.clean")}
      </button>
      <button type="button" class="menu-item" onclick={menuRun(() => requestRefAction({ kind: "backups" }))}>
        {t("refs.menu.backups")}
      </button>
    {/if}
  </div>
{/if}

<style>
  .menu-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    border-radius: calc(var(--radius-md) - 2px);
    padding: 0.3rem 0.5rem;
    font-size: 13px;
    text-align: left;
  }
  .menu-item:hover {
    background: var(--color-accent);
  }
  .menu-item:disabled {
    opacity: 0.45;
  }
</style>
