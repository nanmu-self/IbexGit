<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import { VirtualList } from "$lib/components/ui/virtual-list";
  import { EmptyState } from "$lib/components/ui/empty-state";
  import { t } from "$lib/i18n";
  import type { ConflictSummary, ConflictType, FileStatus } from "$lib/git";
  import CirclePlus from "@lucide/svelte/icons/circle-plus";
  import CircleMinus from "@lucide/svelte/icons/circle-minus";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import EyeOff from "@lucide/svelte/icons/eye-off";
  import Package from "@lucide/svelte/icons/package";

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
    if (file.conflict) {
      return { letter: "!", cls: "bg-red-600 text-white", title: file.status };
    }
    const s = file.status;
    if (file.untracked || s.startsWith("?"))
      return { letter: "U", cls: "bg-green-600/90 text-white", title: "untracked" };
    if (s.includes("D")) return { letter: "D", cls: "bg-red-500/90 text-white", title: "deleted" };
    if (s.includes("R") || s.includes("C"))
      return { letter: "R", cls: "bg-blue-500/90 text-white", title: "renamed/copied" };
    if (s.includes("A")) return { letter: "A", cls: "bg-green-500/90 text-white", title: "added" };
    if (s.includes("M"))
      return { letter: "M", cls: "bg-amber-500/90 text-white", title: "modified" };
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

<div class="min-h-0 flex-1 overflow-y-auto pb-2">
  <!-- 冲突分区 -->
  {#if conflicts.length > 0}
    <div class="flex items-center gap-2 px-3 py-1.5 text-xs font-medium text-red-500">
      <span>{t("workspace.conflicts")}</span>
      <span class="rounded-full bg-red-500/15 px-1.5 text-[11px] leading-4">{conflicts.length}</span>
      <Button
        variant="ghost"
        size="xs"
        class="ml-auto text-[11px] text-red-500/90 hover:text-red-500"
        onclick={() => ondiscard(conflicts.map((f) => f.path), "all")}
      >
        {t("workspace.discardAll")}
      </Button>
    </div>
    {#snippet conflictRow(file: FileStatus, index: number)}
      {@const key = keyOf("worktree", file.path)}
      {@const chip = statusChip(file)}
      {@const { dir, name } = splitPath(file.path)}
      <div
        role="button"
        tabindex="-1"
        data-row-key={key}
        class="group flex h-full cursor-pointer items-center gap-2 px-3 text-[13px] transition-colors duration-[120ms] ease-out {activeKey === key ? 'bg-accent' : selection.has(key) ? 'bg-accent/60' : 'hover:bg-accent/50'}"
        onclick={(e) => onrowclick(file, "worktree", e)}
        onkeydown={(e) => {
          if (e.key === "Enter") onrowclick(file, "worktree", new MouseEvent("click"));
        }}
        oncontextmenu={(e) => onrowcontext(file, "worktree", e)}
      >
        <span class="flex size-4 shrink-0 animate-pulse items-center justify-center rounded-sm text-[10px] font-bold {chip.cls}">
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
        <span class="text-[10px] text-red-500/80">{file.status}</span>
        <button
          type="button"
          class="invisible rounded p-0.5 text-red-500/70 hover:bg-red-500/10 hover:text-red-500 group-hover:visible"
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
    <VirtualList items={conflicts} itemHeight={ROW} row={conflictRow} getKey={(f) => `c:${f.path}`} />
  {/if}

  <!-- 已暂存分区 -->
  <div class="flex items-center gap-2 px-3 py-1.5 text-xs font-medium text-muted-foreground">
    <span>{t("workspace.staged")}</span>
    <span class="rounded-full bg-muted px-1.5 text-[11px] leading-4">{staged.length}</span>
    {#if staged.length > 0}
      <Button
        variant="ghost"
        size="xs"
        class="ml-auto text-[11px] text-muted-foreground"
        onclick={() => {
          const sel = selectedPaths(staged, "staged");
          onunstage(sel.length > 0 ? sel : staged.map((f) => f.path));
        }}
      >
        {selectedPaths(staged, "staged").length > 0
          ? t("workspace.unstageSelected", { n: selectedPaths(staged, "staged").length })
          : t("workspace.unstageAll")}
      </Button>
    {/if}
  </div>
  {#if staged.length > 0}
    {#snippet stagedRow(file: FileStatus, index: number)}
      {@const key = keyOf("staged", file.path)}
      {@const chip = statusChip(file)}
      {@const { dir, name } = splitPath(file.path)}
      <div
        role="button"
        tabindex="-1"
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
          class="invisible rounded p-0.5 hover:bg-muted group-hover:visible"
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
    <VirtualList items={staged} itemHeight={ROW} row={stagedRow} getKey={(f) => `s:${f.path}`} />
  {/if}

  <!-- 未暂存分区 -->
  <div class="mt-1 flex items-center gap-2 px-3 py-1.5 text-xs font-medium text-muted-foreground">
    <span>{t("workspace.unstaged")}</span>
    <span class="rounded-full bg-muted px-1.5 text-[11px] leading-4">{unstaged.length}</span>
    {#if unstaged.length > 0}
      <div class="ml-auto flex items-center gap-1">
        <Button
          variant="ghost"
          size="xs"
          class="text-[11px] text-red-500/90 hover:text-red-500"
          onclick={() => {
            const sel = selectedPaths(unstaged, "worktree");
            ondiscard(sel.length > 0 ? sel : unstaged.map((f) => f.path), "worktree");
          }}
        >
          {selectedPaths(unstaged, "worktree").length > 0
            ? t("workspace.discardSelected", { n: selectedPaths(unstaged, "worktree").length })
            : t("workspace.discardAll")}
        </Button>
        <Button
          variant="ghost"
          size="xs"
          class="text-[11px] text-muted-foreground"
          onclick={() => {
            const sel = selectedPaths(unstaged, "worktree");
            onstage(sel.length > 0 ? sel : unstaged.map((f) => f.path));
          }}
        >
          {selectedPaths(unstaged, "worktree").length > 0
            ? t("workspace.stageSelected", { n: selectedPaths(unstaged, "worktree").length })
            : t("workspace.stageAll")}
        </Button>
      </div>
    {/if}
  </div>
  {#if unstaged.length > 0}
    {#snippet unstagedRow(file: FileStatus, index: number)}
      {@const key = keyOf("worktree", file.path)}
      {@const chip = statusChip(file)}
      {@const { dir, name } = splitPath(file.path)}
      <div
        role="button"
        tabindex="-1"
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
          class="invisible rounded p-0.5 text-red-500/70 hover:bg-red-500/10 hover:text-red-500 group-hover:visible"
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
          class="invisible rounded p-0.5 hover:bg-muted group-hover:visible"
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
    <VirtualList items={unstaged} itemHeight={ROW} row={unstagedRow} getKey={(f) => `u:${f.path}`} />
  {/if}

  {#if conflicts.length === 0 && staged.length === 0 && unstaged.length === 0}
    <EmptyState
      title={filtered ? t("workspace.noMatch") : t("workspace.empty")}
      hint={filtered ? "" : t("workspace.emptyHint")}
      compact
    />
  {/if}
</div>
