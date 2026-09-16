<script lang="ts">
  /**
   * ConflictEditor (P8): consumes the Rust ConflictModel only. Routes by
   * ConflictType — text conflicts open the CodeMirror editor (dynamic
   * import, ADR-010); Binary/DeleteModify/AddAdd/Rename/DirectoryFile get
   * side actions (ours / theirs / delete) with an optional side-by-side
   * content view.
   */
  import { Button } from "$lib/components/ui/button";
  import { ConfirmDialog } from "$lib/components/ui/confirm-dialog";
  import { t } from "$lib/i18n";
  import { git, normalizeError, type ConflictModel, type ConflictType } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import { settings } from "$lib/stores/settings.svelte";
  import Check from "@lucide/svelte/icons/check";
  import ChevronLeft from "@lucide/svelte/icons/chevron-left";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import ExternalTool from "@lucide/svelte/icons/wrench";
  import FileWarning from "@lucide/svelte/icons/file-warning";
  import GitCompareArrows from "@lucide/svelte/icons/git-compare-arrows";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  let {
    repoId,
    path,
    /** Incremented by the parent on refresh → reload the model. */
    revision = 0,
    onresolved,
    onrefresh,
  }: {
    repoId: string;
    path: string;
    revision?: number;
    onresolved: () => void;
    onrefresh: () => void;
  } = $props();

  let model = $state<ConflictModel | null>(null);
  let loading = $state(true);
  let busy = $state(false);
  let remaining = $state(0);
  let currentBlock = $state(0);
  let compareOpen = $state(false);
  let editorText = $state<string | null>(null);
  /** Live block count from the editor (shifts as blocks are accepted). */
  let liveBlocks = $state(0);
  let deleteOpen = $state(false);

  const TYPE_KEY: Record<ConflictType, string> = {
    content: "content",
    delete_modify: "delete_modify",
    add_add: "add_add",
    rename: "rename",
    rename_delete: "rename_delete",
    binary: "binary",
    directory_file: "directory_file",
  };

  // ---- model loading (reload on path change / refresh) ----
  $effect(() => {
    const id = repoId;
    const p = path;
    void revision;
    loading = true;
    model = null;
    compareOpen = false;
    editorText = null;
    liveBlocks = 0;
    currentBlock = 0;
    git
      .conflictModel(id, p)
      .then((m) => {
        model = m;
        remaining = m.block_count;
      })
      .catch((e) => normalizeError(e))
      .finally(() => {
        if (path === p) loading = false;
      });
  });

  // ---- CodeMirror editor (dynamic import, only for text conflicts) ----
  let editorHost = $state<HTMLDivElement | null>(null);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let editorHandle: any = null;

  $effect(() => {
    const m = model;
    const host = editorHost;
    if (!m || !m.editable || !host) return;
    let disposed = false;
    let handle: { destroy(): void } | null = null;
    // CRLF files are converted for CodeMirror and back on resolve.
    const doc = m.crlf ? m.worktree_text!.replace(/\r\n/g, "\n") : m.worktree_text!;
    import("./editor")
      .then(({ createConflictEditor }) =>
        createConflictEditor(host, {
          doc,
          blocks: m.blocks,
          path,
          labels: {
            ours: t("conflict.acceptCurrent"),
            theirs: t("conflict.acceptIncoming"),
            both: t("conflict.acceptBoth"),
          },
          onBlocksChanged: (blocks) => {
            if (!disposed) liveBlocks = blocks.length;
          },
          onChanged: (text) => {
            if (!disposed) editorText = text;
          },
        }),
      )
      .then((h) => {
        if (disposed) {
          h.destroy();
          return;
        }
        handle = h;
        editorHandle = h;
        liveBlocks = h.getBlocks().length;
      })
      .catch((e) => normalizeError(e));
    return () => {
      disposed = true;
      handle?.destroy();
      if (editorHandle === handle) editorHandle = null;
    };
  });

  function navBlock(delta: number): void {
    if (!editorHandle) return;
    const count = liveBlocks;
    if (count === 0) return;
    currentBlock = (currentBlock + delta + count) % count;
    editorHandle.gotoBlock(currentBlock);
  }

  // ---- actions ----
  async function markResolved(): Promise<void> {
    const id = repoId;
    const m = model;
    if (!m || !m.editable || busy) return;
    busy = true;
    try {
      let text = editorText ?? (m.crlf ? m.worktree_text!.replace(/\r\n/g, "\n") : m.worktree_text!);
      if (m.crlf) text = text.replace(/\n/g, "\r\n");
      await git.resolveConflictText(id, m.path, text);
      showToast("success", t("conflict.resolvedToast", { path: m.path }));
      onresolved();
    } catch (e) {
      normalizeError(e);
    } finally {
      busy = false;
    }
  }

  async function resolveSide(action: "ours" | "theirs" | "delete"): Promise<void> {
    const id = repoId;
    const m = model;
    if (!m || busy) return;
    busy = true;
    try {
      await git.resolveConflictSide(id, m.path, action);
      showToast("success", t("conflict.resolvedToast", { path: m.path }));
      onresolved();
    } catch (e) {
      normalizeError(e);
    } finally {
      busy = false;
    }
  }

  // ---- external merge tool (P8 外部工具调用；P10 默认工具持久化) ----
  const TOOL_PRESETS: Array<{ id: string; label: string; cmd?: string }> = [
    { id: "vscode", label: "VS Code", cmd: "code --wait --merge $REMOTE $LOCAL $BASE $MERGED" },
    { id: "meld", label: "Meld" },
    { id: "kdiff3", label: "KDiff3" },
    { id: "p4merge", label: "P4Merge" },
    { id: "vimdiff", label: "Vimdiff" },
  ];
  let toolMenuOpen = $state(false);

  /** Tool from the settings center (label for the menu default entry). */
  const defaultTool = $derived.by(() => {
    const id = settings.mergeToolId;
    if (!id) return null;
    if (id === "custom") {
      return settings.mergeToolCmd
        ? { label: t("settings.tools.custom"), tool: undefined, cmd: settings.mergeToolCmd }
        : null;
    }
    const preset = TOOL_PRESETS.find((p) => p.id === id);
    return preset
      ? { label: preset.label, tool: preset.cmd ? undefined : preset.id, cmd: preset.cmd }
      : null;
  });

  async function runTool(preset: (typeof TOOL_PRESETS)[number]): Promise<void> {
    const id = repoId;
    const m = model;
    if (!m || busy) return;
    toolMenuOpen = false;
    busy = true;
    showToast("info", t("conflict.mergetoolRunning", { tool: preset.label }));
    try {
      await git.mergetool(id, m.path, preset.cmd ? undefined : preset.id, preset.cmd);
      showToast("success", t("conflict.mergetoolDone", { tool: preset.label }));
      onrefresh();
    } catch (e) {
      normalizeError(e);
      showToast("warning", t("conflict.mergetoolFailed"));
    } finally {
      busy = false;
    }
  }

  const canCompare = $derived(
    model !== null &&
      (model.current_text !== null || model.incoming_text !== null || model.base_text !== null),
  );
</script>

<div class="flex min-h-0 flex-1 flex-col">
  {#snippet pane(title: string, text: string, cls: string)}
    <div class="flex min-w-0 flex-col bg-background">
      <div class="border-b px-2 py-1 text-[11px] font-medium text-muted-foreground {cls}">{title}</div>
      <pre class="overflow-auto p-2 font-mono text-xs leading-5">{text}</pre>
    </div>
  {/snippet}

  <!-- header -->
  <div class="flex min-h-10 flex-wrap items-center gap-2 border-b px-3 py-1.5">
    <span class="text-red-500">!</span>
    <span class="min-w-0 flex-1 truncate font-mono text-[13px]" title={path}>{path}</span>
    {#if model}
      <span class="rounded bg-red-500/15 px-1.5 py-0.5 text-[11px] text-red-600 dark:text-red-400">
        {t(`conflict.type.${TYPE_KEY[model.conflict_type]}`)}
      </span>
      <span class="rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground">
        {model.code}
      </span>
    {/if}

    {#if model?.editable}
      {#if model.block_count > 0}
        <div class="flex items-center gap-1">
          <Button variant="ghost" size="icon-sm" title={t("conflict.prev")} onclick={() => navBlock(-1)}>
            <ChevronLeft class="size-4" />
          </Button>
          <span class="min-w-14 text-center text-xs tabular-nums text-muted-foreground">
            {liveBlocks > 0 ? `${currentBlock + 1}/${liveBlocks}` : t("conflict.noBlocks")}
          </span>
          <Button variant="ghost" size="icon-sm" title={t("conflict.next")} onclick={() => navBlock(1)}>
            <ChevronRight class="size-4" />
          </Button>
        </div>
      {/if}
      <Button variant="ghost" size="icon-sm" title={t("conflict.mergetool")} onclick={() => (toolMenuOpen = !toolMenuOpen)}>
        <ExternalTool class="size-4" />
      </Button>
      <Button size="sm" class="h-7 text-xs" disabled={busy} onclick={markResolved}>
        {#if busy}
          <LoaderCircle class="size-3.5 animate-spin" />
        {:else}
          <Check class="size-3.5" />
        {/if}
        {t("conflict.markResolved")}
      </Button>
    {/if}

    {#if model && !model.submodule && !model.directory && model.conflict_type !== "content"}
      <!-- 选边动作：Modify/Delete、Add/Add、Binary、Rename 等；可编辑的内容冲突
           不重复提供（编辑器 Accept 即选边）。 -->
      <Button variant="outline" size="sm" class="h-7 text-xs" disabled={busy} onclick={() => resolveSide("ours")}>
        {t("conflict.keepOurs")}
      </Button>
      {#if model.has_incoming}
        <Button variant="outline" size="sm" class="h-7 text-xs" disabled={busy} onclick={() => resolveSide("theirs")}>
          {t("conflict.keepTheirs")}
        </Button>
      {/if}
      <Button variant="outline" size="sm" class="h-7 text-xs text-red-500 hover:text-red-500" disabled={busy} onclick={() => (deleteOpen = true)}>
        {t("conflict.delete")}
      </Button>
    {/if}

    {#if canCompare && model && !model.submodule}
      <Button
        variant="ghost"
        size="sm"
        class="h-7 text-xs"
        onclick={() => (compareOpen = !compareOpen)}
      >
        <GitCompareArrows class="size-3.5" />
        {t("conflict.viewSides")}
      </Button>
    {/if}

    {#if model && (model.directory || model.submodule)}
      <span class="flex items-center gap-1.5 text-xs text-muted-foreground">
        <FileWarning class="size-3.5" />
        {t(model.directory ? "conflict.directoryHint" : "conflict.submoduleHint")}
      </span>
      {#if !model.directory}
        <Button variant="outline" size="sm" class="h-7 text-xs text-red-500 hover:text-red-500" disabled={busy} onclick={() => (deleteOpen = true)}>
          {t("conflict.delete")}
        </Button>
      {/if}
    {/if}
  </div>

  <!-- external tool dropdown -->
  {#if toolMenuOpen && model?.editable}
    <div class="border-b bg-muted/40 px-3 py-2">
      <div class="mb-1.5 text-xs text-muted-foreground">{t("conflict.mergetoolPick")}</div>
      <div class="flex flex-wrap gap-1.5">
        {#if defaultTool}
          <Button
            variant="outline"
            size="xs"
            disabled={busy}
            onclick={() =>
              runTool({ id: "default", label: defaultTool.label, cmd: defaultTool.cmd })}
          >
            {t("conflict.mergetoolDefault", { tool: defaultTool.label })}
          </Button>
        {/if}
        {#each TOOL_PRESETS as preset (preset.id)}
          <Button variant="outline" size="xs" disabled={busy} onclick={() => runTool(preset)}>
            {preset.label}
          </Button>
        {/each}
      </div>
    </div>
  {/if}

  <!-- body -->
  {#if loading}
    <div class="flex flex-1 items-center justify-center text-muted-foreground">
      <LoaderCircle class="size-5 animate-spin" />
    </div>
  {:else if model?.editable}
    {#if compareOpen}
      <div class="grid min-h-0 flex-1 grid-cols-2 gap-px overflow-auto bg-border md:grid-cols-3">
        {#if model.base_text !== null}
          <pane title={t("conflict.sideBase")} text={model.base_text} cls="pane-base"></pane>
        {/if}
        {#if model.current_text !== null}
          <pane title={t("conflict.sideOurs")} text={model.current_text} cls="pane-ours"></pane>
        {/if}
        {#if model.incoming_text !== null}
          <pane title={t("conflict.sideTheirs")} text={model.incoming_text} cls="pane-theirs"></pane>
        {/if}
      </div>
    {/if}
    <div class="min-h-0 flex-1 {compareOpen ? 'h-1/2 border-t' : ''}">
      <div bind:this={editorHost} class="cm-conflict-host h-full overflow-auto text-[13px]"></div>
    </div>
  {:else if model && model.oversized}
    <div class="flex flex-1 items-center justify-center px-6 text-center text-sm text-muted-foreground">
      {t("conflict.oversized")}
    </div>
  {:else if model && model.binary}
    <div class="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center text-sm text-muted-foreground">
      <FileWarning class="size-6 text-amber-500" />
      {t("conflict.binaryHint")}
    </div>
  {:else if model}
    <div class="flex flex-1 items-center justify-center px-6 text-center text-sm text-muted-foreground">
      {t("conflict.notEditable")}
    </div>
  {/if}
</div>

<!-- delete confirmation -->
<ConfirmDialog
  bind:open={deleteOpen}
  title={t("conflict.deleteConfirmTitle")}
  description={t("conflict.deleteConfirmDesc", { path })}
  confirmLabel={t("conflict.delete")}
  destructive
  onconfirm={() => resolveSide("delete")}
/>
