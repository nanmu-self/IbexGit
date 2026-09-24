<script lang="ts">
  import { tick } from "svelte";
  import { VirtualList } from "$lib/components/ui/virtual-list";
  import { EmptyState } from "$lib/components/ui/empty-state";
  import { t } from "$lib/i18n";
  import type { ConflictSummary, ConflictType, FileStatus } from "$lib/git";
  import CirclePlus from "@lucide/svelte/icons/circle-plus";
  import CircleMinus from "@lucide/svelte/icons/circle-minus";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import EyeOff from "@lucide/svelte/icons/eye-off";
  import Package from "@lucide/svelte/icons/package";
  import StatusSectionHeader from "./StatusSectionHeader.svelte";

  export interface FileKey {
    source: "worktree" | "staged";
    file: FileStatus;
  }

  let {
    conflicts = [],
    staged = [],
    unstaged = [],
    activeKey = null,
    filtered = false,
    selection,
    conflictMeta = {},
    onrowclick,
    onrowcontext,
    onstage,
    onunstage,
    ondiscard,
  }: {
    conflicts?: FileStatus[];
    staged?: FileStatus[];
    unstaged?: FileStatus[];
    /** Key of the file whose diff is shown (`source:path`). */
    activeKey?: string | null;
    /** A filter is active: empty result gets a different message. */
    filtered?: boolean;
    /** Reactive SvelteSet of selected row keys (`source:path`). */
    selection: Set<string>;
    /** P8: path → conflict classification for badges. */
    conflictMeta?: Record<string, ConflictSummary>;
    onrowclick: (file: FileStatus, source: "worktree" | "staged", e: MouseEvent) => void;
    onrowcontext: (file: FileStatus, source: "worktree" | "staged", e: MouseEvent) => void;
    onstage: (paths: string[]) => void;
    onunstage: (paths: string[]) => void;
    ondiscard: (paths: string[], scope: "worktree" | "all") => void;
  } = $props();

  const ROW = 26;

  function keyOf(source: "worktree" | "staged", path: string): string {
    return `${source}:${path}`;
  }

  // ---- keyboard navigation (listbox pattern, data-driven: the virtualized
  // DOM only holds visible rows, so traversal must use the section data) ----
  let listEl: HTMLDivElement | null = $state(null);
  /** Per-section virtual list handle (keyboard-nav scrolling). */
  type SectionList = { ensureVisible(index: number): void };
  let conflictList: SectionList | null = $state(null);
  let stagedList: SectionList | null = $state(null);
  let unstagedList: SectionList | null = $state(null);

  interface FlatRow {
    file: FileStatus;
    source: "worktree" | "staged";
    section: "conflicts" | "staged" | "unstaged";
    index: number;
  }

  function flatRows(): FlatRow[] {
    const out: FlatRow[] = [];
    conflicts.forEach((f, i) =>
      out.push({ file: f, source: "worktree", section: "conflicts", index: i }),
    );
    staged.forEach((f, i) =>
      out.push({ file: f, source: "staged", section: "staged", index: i }),
    );
    unstaged.forEach((f, i) =>
      out.push({ file: f, source: "worktree", section: "unstaged", index: i }),
    );
    return out;
  }

  /** Guard: keys on the row's inline action buttons must not double-fire. */
  function insideButton(e: Event): boolean {
    return e.target instanceof Element && !!e.target.closest("button");
  }

  /** First row in visual order: the Tab entry point when nothing is active. */
  const firstRowKey = $derived(
    conflicts.length > 0
      ? keyOf("worktree", conflicts[0].path)
      : staged.length > 0
        ? keyOf("staged", staged[0].path)
        : unstaged.length > 0
          ? keyOf("worktree", unstaged[0].path)
          : null,
  );
  const isTabStop = (key: string): boolean =>
    activeKey === key || (activeKey === null && firstRowKey === key);

  function onContainerKeydown(e: KeyboardEvent): void {
    if (insideButton(e)) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      void moveActive(e.key === "ArrowDown" ? 1 : -1);
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      void moveActive(e.key === "Home" ? "first" : "last");
    } else if (e.key === " ") {
      // Space toggles membership in the multi-selection (ctrl-click parity).
      // Target = the focused row (covers Tab entry with no active row yet),
      // falling back to the active row. Without a target we don't
      // preventDefault, but we never let Space scroll as a surprise.
      const active = document.activeElement;
      const focusedKey =
        active instanceof Element ? active.getAttribute("data-row-key") : null;
      const lookup = focusedKey ?? activeKey;
      if (lookup) {
        const rows = flatRows();
        const target = rows.find((r) => keyOf(r.source, r.file.path) === lookup);
        if (target) {
          e.preventDefault();
          onrowclick(target.file, target.source, new MouseEvent("click", { ctrlKey: true }));
        }
      }
    }
  }

  async function moveActive(delta: 1 | -1 | "first" | "last"): Promise<void> {
    const rows = flatRows();
    if (rows.length === 0) return;
    const cur = activeKey
      ? rows.findIndex((r) => keyOf(r.source, r.file.path) === activeKey)
      : -1;
    const next =
      delta === "first"
        ? 0
        : delta === "last"
          ? rows.length - 1
          : cur === -1
            ? delta === 1
              ? 0
              : rows.length - 1
            : Math.min(rows.length - 1, Math.max(0, cur + delta));
    const target = rows[next];
    const key = keyOf(target.source, target.file.path);
    onrowclick(target.file, target.source, new MouseEvent("click"));
    // Scroll the target section's virtual window BEFORE the tick: ensureVisible
    // synchronously updates that VirtualList's internal scrollTop state, so
    // after one tick the target row is guaranteed rendered — focus() can
    // never hit a no-op on an unrendered row. The trailing scrollIntoView is
    // a no-op in the normal flex layout (sections always fit the container);
    // caveat: it CAN programmatically scroll an overflow-hidden ancestor, so
    // if the outer container ever grows real overflow, replace it instead of
    // relying on it (users couldn't scroll back out).
    const list =
      target.section === "conflicts"
        ? conflictList
        : target.section === "staged"
          ? stagedList
          : unstagedList;
    list?.ensureVisible(target.index);
    await tick();
    const el = listEl?.querySelector(`[data-row-key="${CSS.escape(key)}"]`);
    if (el instanceof HTMLElement) {
      el.focus({ preventScroll: true });
      el.scrollIntoView({ block: "nearest" });
    } else {
      listEl?.focus(); // resilience: keep keydown alive if the row vanished
    }
  }

  /** Paths of the current selection that fall inside one section list. */
  function selectedPaths(files: FileStatus[], source: "worktree" | "staged"): string[] {
    return files
      .filter((f) => selection.has(keyOf(source, f.path)))
      .map((f) => f.path);
  }

  function splitPath(path: string): { dir: string; name: string } {
    const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    return idx === -1
      ? { dir: "", name: path }
      : { dir: path.slice(0, idx + 1), name: path.slice(idx + 1) };
  }

  function statusChip(file: FileStatus): { letter: string; cls: string; title: string } {
    // Chip backgrounds sit at 700/600 level so white 10px text keeps ≥4.5:1.
    if (file.conflict) {
      return { letter: "!", cls: "bg-red-600 text-white", title: file.status };
    }
    const s = file.status;
    if (file.untracked || s.startsWith("?"))
      return { letter: "U", cls: "bg-green-700 text-white", title: "untracked" };
    if (s.includes("D")) return { letter: "D", cls: "bg-red-600 text-white", title: "deleted" };
    if (s.includes("R") || s.includes("C"))
      return { letter: "R", cls: "bg-blue-600 text-white", title: "renamed/copied" };
    if (s.includes("A")) return { letter: "A", cls: "bg-green-700 text-white", title: "added" };
    if (s.includes("M"))
      return { letter: "M", cls: "bg-amber-700 text-white", title: "modified" };
    return { letter: "M", cls: "bg-muted-foreground/70 text-white", title: s };
  }

  const TYPE_KEY: Record<ConflictType, string> = {
    content: "content",
    delete_modify: "delete_modify",
    add_add: "add_add",
    rename: "rename",
    rename_delete: "rename_delete",
    binary: "binary",
    directory_file: "directory_file",
  };
</script>

<div
  bind:this={listEl}
  role="listbox"
  aria-label={t("workspace.fileListAria")}
  tabindex="-1"
  class="flex min-h-0 flex-1 flex-col overflow-hidden pb-2"
  onkeydown={onContainerKeydown}
>
  <!-- 冲突分区 -->
  {#if conflicts.length > 0}
    <StatusSectionHeader
      title={t("workspace.conflicts")}
      count={conflicts.length}
      tone="red"
      actions={[
        {
          label: t("workspace.discardAll"),
          danger: true,
          onclick: () => ondiscard(conflicts.map((f) => f.path), "all"),
        },
      ]}
    />
    {#snippet conflictRow(file: FileStatus, index: number)}
      {@const key = keyOf("worktree", file.path)}
      {@const chip = statusChip(file)}
      {@const { dir, name } = splitPath(file.path)}
      <div
        role="option"
        aria-selected={activeKey === key || selection.has(key)}
        tabindex={isTabStop(key) ? 0 : -1}
        data-row-key={key}
        class="group flex h-full cursor-pointer items-center gap-2 px-3 text-[13px] transition-colors duration-[120ms] ease-out {activeKey === key ? 'bg-accent' : selection.has(key) ? 'bg-accent/60' : 'hover:bg-accent/50'}"
        onclick={(e) => onrowclick(file, "worktree", e)}
        onkeydown={(e) => {
          if (e.key === "Enter") onrowclick(file, "worktree", new MouseEvent("click"));
        }}
        oncontextmenu={(e) => onrowcontext(file, "worktree", e)}
      >
        <span class="flex size-4 shrink-0 items-center justify-center rounded-sm text-[10px] font-bold {chip.cls}">
          {chip.letter}
        </span>
        <span class="min-w-0 flex-1 truncate">
          <span class="text-muted-foreground/60">{dir}</span>{name}
        </span>
        {#if conflictMeta[file.path]}
          {@const meta = conflictMeta[file.path]}
          <span
            class="rounded bg-red-500/10 px-1.5 py-0.5 text-[10px] text-red-600 dark:text-red-400"
            title={t(`conflict.type.${TYPE_KEY[meta.conflict_type]}`)}
          >
            {t(`conflict.type.${TYPE_KEY[meta.conflict_type]}`)}
          </span>
          {#if meta.block_count > 0}
            <span
              class="rounded-full bg-red-500/15 px-1.5 text-[10px] leading-4 text-red-500"
              title={t("conflict.blocksN", { n: meta.block_count })}
            >
              {meta.block_count}
            </span>
          {/if}
        {/if}
        <button
          type="button"
          class="invisible grid size-6 shrink-0 place-items-center rounded text-red-500/70 hover:bg-red-500/10 hover:text-red-500 group-hover:visible group-focus-within:visible"
          title={t("workspace.discard")}
          onclick={(e) => {
            e.stopPropagation();
            ondiscard([file.path], "all");
          }}
        >
          <Trash2 class="size-3.5" />
        </button>
      </div>
    {/snippet}
    <VirtualList
      bind:this={conflictList}
      sizing="content"
      items={conflicts}
      itemHeight={ROW}
      row={conflictRow}
      getKey={(f) => `c:${f.path}`}
    />
  {/if}

  <!-- 已暂存分区 -->
  <StatusSectionHeader
    title={t("workspace.staged")}
    count={staged.length}
    actions={staged.length > 0
      ? [
          {
            label:
              selectedPaths(staged, "staged").length > 0
                ? t("workspace.unstageSelected", {
                    n: selectedPaths(staged, "staged").length,
                  })
                : t("workspace.unstageAll"),
            onclick: () => {
              const sel = selectedPaths(staged, "staged");
              onunstage(sel.length > 0 ? sel : staged.map((f) => f.path));
            },
          },
        ]
      : []}
  />
  {#if staged.length > 0}
    {#snippet stagedRow(file: FileStatus, index: number)}
      {@const key = keyOf("staged", file.path)}
      {@const chip = statusChip(file)}
      {@const { dir, name } = splitPath(file.path)}
      <div
        role="option"
        aria-selected={activeKey === key || selection.has(key)}
        tabindex={isTabStop(key) ? 0 : -1}
        data-row-key={key}
        class="group flex h-full cursor-pointer items-center gap-2 px-3 text-[13px] transition-colors duration-[120ms] ease-out {activeKey === key ? 'bg-accent' : selection.has(key) ? 'bg-accent/60' : 'hover:bg-accent/50'}"
        onclick={(e) => onrowclick(file, "staged", e)}
        onkeydown={(e) => {
          if (e.key === "Enter") onrowclick(file, "staged", new MouseEvent("click"));
        }}
        oncontextmenu={(e) => onrowcontext(file, "staged", e)}
      >
        <span class="flex size-4 shrink-0 items-center justify-center rounded-sm text-[10px] font-bold {chip.cls}">
          {chip.letter}
        </span>
        <span class="min-w-0 flex-1 truncate" title={file.orig_path ? `${file.orig_path} → ${file.path}` : file.path}>
          <span class="text-muted-foreground/60">{dir}</span>{name}
        </span>
        {#if file.submodule}
          <span class="flex items-center gap-0.5 text-[10px] text-violet-500" title={file.submodule_commit_changed ? t("workspace.subCommit") : file.submodule_dirty ? t("workspace.subDirty") : t("workspace.submodule")}>
            <Package class="size-3" />
            {#if file.submodule_commit_changed}↕{:else if file.submodule_dirty}!{/if}
          </span>
        {/if}
        {#if file.skipped}
          <EyeOff class="size-3 shrink-0 text-muted-foreground/60" title={t("workspace.skipWorktree")} />
        {/if}
        {#if file.eol_only}
          <span class="shrink-0 rounded bg-muted px-1 text-[9px] leading-4 text-muted-foreground" title={t("workspace.eolOnly")}>EOL</span>
        {/if}
        <button
          type="button"
          class="invisible grid size-6 shrink-0 place-items-center rounded hover:bg-muted group-hover:visible group-focus-within:visible"
          title={t("workspace.unstage")}
          onclick={(e) => {
            e.stopPropagation();
            onunstage([file.path]);
          }}
        >
          <CircleMinus class="size-3.5" />
        </button>
      </div>
    {/snippet}
    <VirtualList
      bind:this={stagedList}
      sizing="content"
      items={staged}
      itemHeight={ROW}
      row={stagedRow}
      getKey={(f) => `s:${f.path}`}
    />
  {/if}

  <!-- 未暂存分区 -->
  <div class="mt-1 shrink-0">
    <StatusSectionHeader
      title={t("workspace.unstaged")}
      count={unstaged.length}
      actions={unstaged.length > 0
        ? [
            {
              label:
                selectedPaths(unstaged, "worktree").length > 0
                  ? t("workspace.discardSelected", {
                      n: selectedPaths(unstaged, "worktree").length,
                    })
                  : t("workspace.discardAll"),
              danger: true,
              onclick: () => {
                const sel = selectedPaths(unstaged, "worktree");
                ondiscard(sel.length > 0 ? sel : unstaged.map((f) => f.path), "worktree");
              },
            },
            {
              label:
                selectedPaths(unstaged, "worktree").length > 0
                  ? t("workspace.stageSelected", {
                      n: selectedPaths(unstaged, "worktree").length,
                    })
                  : t("workspace.stageAll"),
              onclick: () => {
                const sel = selectedPaths(unstaged, "worktree");
                onstage(sel.length > 0 ? sel : unstaged.map((f) => f.path));
              },
            },
          ]
        : []}
    />
  </div>
  {#if unstaged.length > 0}
    {#snippet unstagedRow(file: FileStatus, index: number)}
      {@const key = keyOf("worktree", file.path)}
      {@const chip = statusChip(file)}
      {@const { dir, name } = splitPath(file.path)}
      <div
        role="option"
        aria-selected={activeKey === key || selection.has(key)}
        tabindex={isTabStop(key) ? 0 : -1}
        data-row-key={key}
        class="group flex h-full cursor-pointer items-center gap-2 px-3 text-[13px] transition-colors duration-[120ms] ease-out {activeKey === key ? 'bg-accent' : selection.has(key) ? 'bg-accent/60' : 'hover:bg-accent/50'}"
        onclick={(e) => onrowclick(file, "worktree", e)}
        onkeydown={(e) => {
          if (e.key === "Enter") onrowclick(file, "worktree", new MouseEvent("click"));
        }}
        oncontextmenu={(e) => onrowcontext(file, "worktree", e)}
      >
        <span class="flex size-4 shrink-0 items-center justify-center rounded-sm text-[10px] font-bold {chip.cls}">
          {chip.letter}
        </span>
        <span class="min-w-0 flex-1 truncate" title={file.orig_path ? `${file.orig_path} → ${file.path}` : file.path}>
          <span class="text-muted-foreground/60">{dir}</span>{name}
        </span>
        {#if file.submodule}
          <span class="flex items-center gap-0.5 text-[10px] text-violet-500" title={file.submodule_commit_changed ? t("workspace.subCommit") : file.submodule_dirty ? t("workspace.subDirty") : t("workspace.submodule")}>
            <Package class="size-3" />
            {#if file.submodule_commit_changed}↕{:else if file.submodule_dirty}!{/if}
          </span>
        {/if}
        {#if file.skipped}
          <EyeOff class="size-3 shrink-0 text-muted-foreground/60" title={t("workspace.skipWorktree")} />
        {/if}
        {#if file.eol_only}
          <span class="shrink-0 rounded bg-muted px-1 text-[9px] leading-4 text-muted-foreground" title={t("workspace.eolOnly")}>EOL</span>
        {/if}
        <button
          type="button"
          class="invisible grid size-6 shrink-0 place-items-center rounded text-red-500/70 hover:bg-red-500/10 hover:text-red-500 group-hover:visible group-focus-within:visible"
          title={t("workspace.discard")}
          onclick={(e) => {
            e.stopPropagation();
            ondiscard([file.path], file.conflict ? "all" : file.untracked ? "worktree" : "worktree");
          }}
        >
          <Trash2 class="size-3.5" />
        </button>
        <button
          type="button"
          class="invisible grid size-6 shrink-0 place-items-center rounded hover:bg-muted group-hover:visible group-focus-within:visible"
          title={t("workspace.stage")}
          onclick={(e) => {
            e.stopPropagation();
            onstage([file.path]);
          }}
        >
          <CirclePlus class="size-3.5" />
        </button>
      </div>
    {/snippet}
    <VirtualList
      bind:this={unstagedList}
      sizing="content"
      items={unstaged}
      itemHeight={ROW}
      row={unstagedRow}
      getKey={(f) => `u:${f.path}`}
    />
  {/if}

  {#if conflicts.length === 0 && staged.length === 0 && unstaged.length === 0}
    <EmptyState
      title={filtered ? t("workspace.noMatch") : t("workspace.empty")}
      hint={filtered ? "" : t("workspace.emptyHint")}
      compact
    />
  {/if}
</div>
