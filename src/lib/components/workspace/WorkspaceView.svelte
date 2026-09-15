<script lang="ts">
  import { Input } from "$lib/components/ui/input";
  import { Button } from "$lib/components/ui/button";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { VirtualList } from "$lib/components/ui/virtual-list";
  import { EmptyState } from "$lib/components/ui/empty-state";
  import { PanelResizer } from "$lib/components/ui/panel-resizer";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { t } from "$lib/i18n";
  import { repos, splitFiles } from "$lib/stores/repos.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { git, normalizeError, type FileStatus, type DiffModel } from "$lib/git";
  import { onAction } from "$lib/keyboard";
  import CirclePlus from "@lucide/svelte/icons/circle-plus";
  import CircleMinus from "@lucide/svelte/icons/circle-minus";
  import Trash2 from "@lucide/svelte/icons/trash-2";

  const ROW = 26;

  const filter = $derived(repos.ui.filter.trim().toLowerCase());
  const active = $derived(repos.active);
  const selected = $derived(repos.ui.selected_file);

  const staged = $derived(
    splitFiles(active?.files ?? []).staged.filter((f) => f.path.toLowerCase().includes(filter))
  );
  const unstaged = $derived(
    splitFiles(active?.files ?? []).unstaged.filter((f) => f.path.toLowerCase().includes(filter))
  );

  let diffModel = $state<DiffModel | null>(null);
  let diffLoading = $state(false);

  // (Re)load the diff whenever the selection or repo state changes.
  $effect(() => {
    const sel = repos.ui.selected_file;
    const tab = repos.active;
    const refreshMark = tab?.lastRefreshMs; // re-fetch after watcher refresh
    if (!sel || !tab) {
      diffModel = null;
      diffLoading = false;
      return;
    }
    diffLoading = true;
    git
      .diff(tab.id, sel.source, undefined, undefined, [sel.path])
      .then((model) => {
        const cur = repos.ui.selected_file;
        if (cur?.path === sel.path && cur.source === sel.source) diffModel = model;
      })
      .catch((e) => {
        normalizeError(e);
        diffModel = null;
      })
      .finally(() => {
        if (repos.ui.selected_file?.path === sel.path) diffLoading = false;
      });
    void refreshMark;
  });

  // Ctrl+Shift+F jumps to the filter box.
  $effect(() => {
    return onAction("workspace.focusFilter", () => {
      document.querySelector<HTMLInputElement>("input[data-filter]")?.focus();
    });
  });

  function selectFile(file: FileStatus, source: "worktree" | "staged"): void {
    repos.updateUi({ selected_file: { path: file.path, source } });
  }

  async function stage(paths: string[]): Promise<void> {
    const id = repos.activeId;
    if (id === null || paths.length === 0) return;
    try {
      await git.stage(id, paths);
      await repos.refresh(id);
    } catch (e) {
      normalizeError(e);
    }
  }

  async function unstage(paths: string[]): Promise<void> {
    const id = repos.activeId;
    if (id === null || paths.length === 0) return;
    try {
      await git.unstage(id, paths);
      await repos.refresh(id);
    } catch (e) {
      normalizeError(e);
    }
  }

  function splitPath(path: string): { dir: string; name: string } {
    const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    return idx === -1 ? { dir: "", name: path } : { dir: path.slice(0, idx + 1), name: path.slice(idx + 1) };
  }

  function statusChip(file: FileStatus): { letter: string; cls: string } {
    if (file.conflict) return { letter: "!", cls: "bg-red-600 text-white" };
    const s = file.status;
    if (file.untracked || s.startsWith("?")) return { letter: "U", cls: "bg-green-600/90 text-white" };
    if (s.includes("D")) return { letter: "D", cls: "bg-red-500/90 text-white" };
    if (s.includes("R") || s.includes("C")) return { letter: "R", cls: "bg-blue-500/90 text-white" };
    if (s.includes("A")) return { letter: "A", cls: "bg-green-500/90 text-white" };
    if (s.includes("M")) return { letter: "M", cls: "bg-amber-500/90 text-white" };
    return { letter: "M", cls: "bg-muted-foreground/70 text-white" };
  }
</script>

<div class="flex min-h-0 flex-1">
  <!-- 文件列表 -->
  <div class="flex min-h-0 flex-col border-r" style="width: {settings.fileListWidth}px">
    <div class="p-2">
      <Input
        data-filter
        placeholder={t("workspace.filter")}
        bind:value={repos.ui.filter}
        class="h-8 bg-background text-[13px]"
      />
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto pb-2">
      <!-- 已暂存 -->
      <div class="flex items-center gap-2 px-3 py-1.5 text-xs font-medium text-muted-foreground">
        <span>{t("workspace.staged")}</span>
        <span class="rounded-full bg-muted px-1.5 text-[11px] leading-4">{staged.length}</span>
        {#if staged.length > 0}
          <Button
            variant="ghost"
            size="sm"
            class="ml-auto h-6 px-2 text-[11px] text-muted-foreground"
            onclick={() => unstage(staged.map((f) => f.path))}
          >
            {t("workspace.unstageAll")}
          </Button>
        {/if}
      </div>
      {#if staged.length > 0}
        {#snippet stagedRow(file: FileStatus, index: number)}
          {@const chip = statusChip(file)}
          {@const { dir, name } = splitPath(file.path)}
          <div
            role="button"
            tabindex="-1"
            class="group flex h-full cursor-pointer items-center gap-2 px-3 text-[13px] {selected?.path === file.path && selected?.source === 'staged'
              ? 'bg-accent'
              : 'hover:bg-accent/50'}"
            onclick={() => selectFile(file, "staged")}
            onkeydown={(e) => {
              if (e.key === "Enter") selectFile(file, "staged");
            }}
          >
            <span class="flex size-4 shrink-0 items-center justify-center rounded-sm text-[10px] font-bold {chip.cls}">
              {chip.letter}
            </span>
            <span class="min-w-0 flex-1 truncate" title={file.orig_path ? `${file.orig_path} → ${file.path}` : file.path}>
              <span class="text-muted-foreground/60">{dir}</span>{name}
            </span>
            <button
              type="button"
              class="invisible rounded p-0.5 hover:bg-muted group-hover:visible"
              title={t("workspace.unstage")}
              onclick={(e) => {
                e.stopPropagation();
                void unstage([file.path]);
              }}
            >
              <CircleMinus class="size-3.5" />
            </button>
          </div>
        {/snippet}
        <VirtualList items={staged} itemHeight={ROW} row={stagedRow} getKey={(f) => `s:${f.path}`} />
      {/if}

      <!-- 未暂存 -->
      <div class="mt-1 flex items-center gap-2 px-3 py-1.5 text-xs font-medium text-muted-foreground">
        <span>{t("workspace.unstaged")}</span>
        <span class="rounded-full bg-muted px-1.5 text-[11px] leading-4">{unstaged.length}</span>
        {#if unstaged.length > 0}
          <Button
            variant="ghost"
            size="sm"
            class="ml-auto h-6 px-2 text-[11px] text-muted-foreground"
            onclick={() => stage(unstaged.map((f) => f.path))}
          >
            {t("workspace.stageAll")}
          </Button>
        {/if}
      </div>
      {#if unstaged.length > 0}
        {#snippet unstagedRow(file: FileStatus, index: number)}
          {@const chip = statusChip(file)}
          {@const { dir, name } = splitPath(file.path)}
          <div
            role="button"
            tabindex="-1"
            class="group flex h-full cursor-pointer items-center gap-2 px-3 text-[13px] {selected?.path === file.path && selected?.source === 'worktree'
              ? 'bg-accent'
              : 'hover:bg-accent/50'}"
            onclick={() => selectFile(file, "worktree")}
            onkeydown={(e) => {
              if (e.key === "Enter") selectFile(file, "worktree");
            }}
          >
            <span class="flex size-4 shrink-0 items-center justify-center rounded-sm text-[10px] font-bold {chip.cls}">
              {chip.letter}
            </span>
            <span class="min-w-0 flex-1 truncate" title={file.orig_path ? `${file.orig_path} → ${file.path}` : file.path}>
              <span class="text-muted-foreground/60">{dir}</span>{name}
            </span>
            <button
              type="button"
              class="invisible rounded p-0.5 text-muted-foreground/50 hover:bg-muted hover:text-foreground group-hover:visible"
              title={t("workspace.discard")}
              disabled
            >
              <Trash2 class="size-3.5" />
            </button>
            <button
              type="button"
              class="invisible rounded p-0.5 hover:bg-muted group-hover:visible"
              title={t("workspace.stage")}
              onclick={(e) => {
                e.stopPropagation();
                void stage([file.path]);
              }}
            >
              <CirclePlus class="size-3.5" />
            </button>
          </div>
        {/snippet}
        <VirtualList items={unstaged} itemHeight={ROW} row={unstagedRow} getKey={(f) => `u:${f.path}`} />
      {/if}

      {#if staged.length === 0 && unstaged.length === 0 && active}
        <EmptyState
          title={filter ? t("workspace.noMatch") : t("workspace.empty")}
          hint={filter ? "" : t("workspace.emptyHint")}
          compact
        />
      {/if}
    </div>
  </div>

  <PanelResizer bind:width={settings.fileListWidth} min={220} max={560} />

  <!-- 差异 + 提交框（提交在 P3 启用） -->
  <div class="flex min-w-0 flex-1 flex-col">
    <div class="min-h-0 flex-1">
      <DiffViewer model={diffModel} loading={diffLoading} />
    </div>

    <div class="space-y-2 border-t bg-background p-2.5">
      <Input placeholder={t("commit.subject")} disabled class="h-8 text-[13px]" />
      <textarea
        rows={2}
        placeholder={t("commit.description")}
        disabled
        class="w-full resize-none rounded-md border border-input bg-transparent px-3 py-2 text-[13px] opacity-60 placeholder:text-muted-foreground"
      ></textarea>
      <div class="flex items-center gap-4 text-xs text-muted-foreground">
        <label class="flex items-center gap-1.5 opacity-60">
          <Checkbox disabled />
          {t("commit.amend")}
        </label>
        <label class="flex items-center gap-1.5 opacity-60">
          <Checkbox disabled />
          {t("commit.noVerify")}
        </label>
        <label class="flex items-center gap-1.5 opacity-60">
          <Checkbox disabled />
          {t("commit.andPush")}
        </label>
        <Button disabled class="ml-auto h-8 min-w-36 text-xs" title={t("commit.soon")}>
          {t("commit.needsMessage")}
        </Button>
      </div>
    </div>
  </div>
</div>
