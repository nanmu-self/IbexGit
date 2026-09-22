<script lang="ts" module>
  import { slide } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { browser } from "$app/environment";

  // The operation banner is rare but high-stakes: collapse its height so the
  // workspace list glides down instead of jumping 28px.
  const reduceMotion =
    browser && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const bannerCollapse = reduceMotion
    ? { duration: 80, easing: cubicOut }
    : { duration: 240, easing: cubicOut };
</script>

<script lang="ts">
  import { Input } from "$lib/components/ui/input";
  import { Button } from "$lib/components/ui/button";
  import { ConfirmDialog } from "$lib/components/ui/confirm-dialog";
  import { PanelResizer } from "$lib/components/ui/panel-resizer";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import ConflictEditor from "$lib/components/merge/ConflictEditor.svelte";
  import OperationBanner from "$lib/components/merge/OperationBanner.svelte";
  import StatusSections from "./StatusSections.svelte";
  import StatusTree from "./StatusTree.svelte";
  import CommitBox from "./CommitBox.svelte";
  import RecoveryDialog from "./RecoveryDialog.svelte";
  import FileContextMenu from "./FileContextMenu.svelte";
  import type {
    ContextTarget,
  } from "./FileContextMenu.svelte";
  import { t } from "$lib/i18n";
  import { repos, splitFiles } from "$lib/stores/repos.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import {
    git,
    app,
    recovery,
    normalizeError,
    type ConflictSummary,
    type FileStatus,
    type DiffModel,
    type LineSelection,
    type RecoveryEntry,
  } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import { runPush } from "$lib/stores/netops.svelte";
  import { fileView } from "$lib/stores/fileview.svelte";
  import { onAction } from "$lib/keyboard";
  import { revealItemInDir } from "@tauri-apps/plugin-opener";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import List from "@lucide/svelte/icons/list";
  import FolderTree from "@lucide/svelte/icons/folder-tree";
  import History from "@lucide/svelte/icons/history";

  type Source = "worktree" | "staged";

  const filter = $derived(repos.ui.filter.trim().toLowerCase());
  const active = $derived(repos.active);
  const activeKey = $derived(
    repos.ui.selected_file
      ? `${repos.ui.selected_file.source}:${repos.ui.selected_file.path}`
      : null
  );

  // ---- P8 conflict/operation state ----
  const conflictMeta = $derived.by(() => {
    const map: Record<string, ConflictSummary> = {};
    for (const c of active?.conflicts ?? []) map[c.path] = c;
    return map;
  });
  const selectedIsConflict = $derived(
    repos.ui.selected_file?.source === "worktree" &&
      repos.ui.selected_file.path in conflictMeta,
  );

  // ---- sections (filtered) ----
  const matches = (f: FileStatus): boolean =>
    f.path.toLowerCase().includes(filter);
  const sections = $derived.by(() => {
    const all = splitFiles(active?.files ?? []);
    if (!filter) return all;
    return {
      conflicts: all.conflicts.filter(matches),
      staged: all.staged.filter(matches),
      unstaged: all.unstaged.filter(matches),
    };
  });
  const filteredActive = $derived(filter.length > 0);

  /** Ordered lookup for shift-range selection. */
  const order = $derived.by(() => {
    const map = new Map<Source, string[]>();
    map.set(
      "staged",
      sections.staged.map((f) => f.path)
    );
    map.set(
      "worktree",
      [...sections.conflicts, ...sections.unstaged].map((f) => f.path)
    );
    return map;
  });

  // ---- selection (batch ops) ----
  const selection = $state(new Set<string>());
  let anchor = $state<string | null>(null);

  function keyOf(source: Source, path: string): string {
    return `${source}:${path}`;
  }

  function onRowClick(file: FileStatus, source: Source, e: MouseEvent): void {
    const key = keyOf(source, file.path);
    if (e.shiftKey && anchor) {
      const list = order.get(source) ?? [];
      const a = list.indexOf(anchor.split(":").slice(1).join(":"));
      const b = list.indexOf(file.path);
      if (a !== -1 && b !== -1) {
        const [lo, hi] = a < b ? [a, b] : [b, a];
        for (let i = lo; i <= hi; i++) selection.add(keyOf(source, list[i]));
        return;
      }
    }
    if (e.ctrlKey || e.metaKey) {
      if (selection.has(key)) selection.delete(key);
      else selection.add(key);
      anchor = key;
      return;
    }
    // Plain click: single selection + diff target.
    selection.clear();
    selection.add(key);
    anchor = key;
    repos.updateUi({ selected_file: { path: file.path, source } });
  }

  function onRowContext(file: FileStatus, source: Source, e: MouseEvent): void {
    e.preventDefault();
    const root = active?.path ?? "";
    ctxTarget = {
      path: file.path,
      untracked: file.untracked,
      absPath: `${root.replace(/[\\/]+$/, "")}/${file.path}`,
      x: e.clientX,
      y: e.clientY,
    };
  }

  // ---- diff loading (re-fetch after watcher refresh) ----
  // P4: context expansion + ignore-whitespace refetch; the expansion state
  // is per selected file and resets when the selection changes.
  let diffModel = $state<DiffModel | null>(null);
  let diffLoading = $state(false);
  let diffIgnoreWs = $state(false);
  let diffContext = $state<{ path: string | null; context: number }>({
    path: null,
    context: 3,
  });

  $effect(() => {
    const sel = repos.ui.selected_file;
    const tab = repos.active;
    const refreshMark = tab?.lastRefreshMs;
    if (!sel || !tab) {
      diffModel = null;
      diffLoading = false;
      return;
    }
    // Conflicted paths render through the ConflictEditor instead.
    if (sel.source === "worktree" && sel.path in conflictMeta) {
      diffModel = null;
      diffLoading = false;
      return;
    }
    // Effective context: the override only applies to the file it was
    // expanded for; switching files falls back to git's default 3.
    const context = diffContext.path === sel.path ? diffContext.context : 3;
    diffLoading = true;
    git
      .diff(tab.id, sel.source, undefined, undefined, [sel.path], context, diffIgnoreWs)
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

  function handleExpand(path: string, dir: "up" | "down" | "all"): void {
    const current = diffContext.path === path ? diffContext.context : 3;
    if (dir === "all") {
      diffContext = { path, context: 100_000 };
    } else {
      diffContext = { path, context: Math.min(current + 10, 100_000) };
    }
  }

  function handleLineOp(
    op: "stage" | "discard" | "unstage",
    path: string,
    selections: LineSelection[]
  ): void {
    const id = repos.activeId;
    const modelId = diffModel?.id ?? 0;
    if (id === null || modelId === 0 || selections.length === 0) return;
    const run = async (): Promise<void> => {
      let snapshotId: string | null = null;
      if (op === "stage") await git.stageLines(id, modelId, path, selections);
      else if (op === "unstage") await git.unstageLines(id, modelId, path, selections);
      else snapshotId = await git.discardLines(id, modelId, path, selections);
      if (snapshotId) {
        showToast(
          "success",
          t("diff.lineDiscardDone"),
          t("workspace.discardUndoHint"),
          8000,
          {
            label: t("workspace.undo"),
            run: () => void undoDiscard(id, snapshotId!),
          },
        );
      }
      await repos.refresh(id);
    };
    run().catch((e) => normalizeError(e));
  }

  let fileListDragging = $state(false);

  function handleIgnoreWs(v: boolean): void {
    diffIgnoreWs = v;
  }

  $effect(() => {
    return onAction("workspace.focusFilter", () => {
      document.querySelector<HTMLInputElement>("input[data-filter]")?.focus();
    });
  });

  // ---- batch actions ----
  const stagedCount = $derived(sections.staged.length);
  const selectionCount = $derived(selection.size);

  async function doStage(paths: string[]): Promise<void> {
    const id = repos.activeId;
    if (id === null || paths.length === 0) return;
    try {
      await git.stage(id, paths);
      await repos.refresh(id);
    } catch (e) {
      normalizeError(e);
    }
  }

  async function doUnstage(paths: string[]): Promise<void> {
    const id = repos.activeId;
    if (id === null || paths.length === 0) return;
    try {
      await git.unstage(id, paths);
      await repos.refresh(id);
    } catch (e) {
      normalizeError(e);
    }
  }

  // Discard: confirm dialog → snapshot-backed backend discard → undo toast.
  let discardPaths = $state<string[]>([]);
  let discardScope = $state<"worktree" | "all">("worktree");
  let discardOpen = $state(false);
  const discardHasUntracked = $derived(
    discardPaths.some((p) => (active?.files ?? []).some((f) => f.path === p && f.untracked))
  );

  function askDiscard(paths: string[], scope: "worktree" | "all"): void {
    if (paths.length === 0) return;
    discardPaths = paths;
    discardScope = scope;
    discardOpen = true;
  }

  async function confirmDiscard(): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    try {
      const snapshotId = await git.discard(id, discardPaths, discardScope);
      selection.clear();
      await repos.refresh(id);
      if (snapshotId) {
        showToast(
          "success",
          t("workspace.discardDone", { n: discardPaths.length }),
          t("workspace.discardUndoHint"),
          8000,
          {
            label: t("workspace.undo"),
            run: () => void undoDiscard(id, snapshotId),
          },
        );
      }
    } catch (e) {
      normalizeError(e);
    }
  }

  async function undoDiscard(id: string, snapshotId: string): Promise<void> {
    try {
      await recovery.restore(id, snapshotId);
      await repos.refresh(id);
      showToast("success", t("workspace.restored"));
    } catch (e) {
      normalizeError(e);
    }
  }

  // ---- context menu ----
  let ctxTarget = $state<ContextTarget | null>(null);
  let ignorePaths = $state<string[]>([]);
  let ignoreOpen = $state(false);

  // P9：文件右键 → 单文件历史 / Blame（冲突行同样走此处，入口打通）。
  function openFileHistory(target: ContextTarget): void {
    fileView.show(target.path, "history");
  }

  function openBlame(target: ContextTarget): void {
    fileView.show(target.path, "blame");
  }

  async function reveal(target: ContextTarget): Promise<void> {
    try {
      await revealItemInDir(target.absPath);
    } catch (e) {
      normalizeError(e);
    }
  }

  async function copyPath(target: ContextTarget): Promise<void> {
    try {
      await writeText(target.absPath);
      showToast("info", t("workspace.ctx.copied"));
    } catch (e) {
      normalizeError(e);
    }
  }

  function askIgnore(target: ContextTarget): void {
    ignorePaths = [target.path];
    ignoreOpen = true;
  }

  async function confirmIgnore(): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    try {
      await git.ignorePaths(id, ignorePaths);
      showToast("success", t("workspace.ignoreDone", { n: ignorePaths.length }));
      await repos.refresh(id);
    } catch (e) {
      normalizeError(e);
    }
  }

  // ---- commit ----
  let commitBusy = $state(false);
  let commitBox = $state<{
    loadMessage: (full: string | null) => void;
  } | null>(null);

  // P8: prefill the commit message from MERGE_MSG while a merge /
  // cherry-pick / revert is in progress (git commit --cleanup=strip
  // removes the "# Conflicts:" comment block on the Rust side). Only
  // load once per distinct message — refresh cycles replace the
  // operation object and must not clobber user edits.
  let lastPrefill = $state<string | null>(null);
  $effect(() => {
    const msg = active?.operation?.message ?? null;
    if (msg && msg !== lastPrefill) {
      lastPrefill = msg;
      commitBox?.loadMessage(msg);
    }
  });

  const commitMode = $derived.by(() => {
    const kind = active?.operation?.kind;
    if (kind === "merge" || kind === "cherry_pick" || kind === "revert") return kind;
    return "normal" as const;
  });

  async function loadHeadMessage(): Promise<string | null> {
    const id = repos.activeId;
    if (id === null) return null;
    try {
      return await git.headMessage(id);
    } catch (e) {
      normalizeError(e);
      return null;
    }
  }

  // ---- P10 提交辅助：commit.template 预填（每仓库一次） ----
  let template = $state<{ repoId: string; content: string } | null>(null);
  $effect(() => {
    const id = active?.id;
    if (!id) return;
    let stale = false;
    app
      .commitTemplate(id)
      .then((tpl) => {
        if (!stale && tpl) template = { repoId: id, content: tpl.content };
      })
      .catch(() => {}); // 未配置 / 不可读 → 不预填
    return () => {
      stale = true;
    };
  });
  const templateForActive = $derived(
    template && active && template.repoId === active.id ? template : null,
  );

  async function handleCommit(
    message: string,
    amend: boolean,
    noVerify: boolean,
    andPush: boolean
  ): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    commitBusy = true;
    try {
      const res = await git.commit(id, message, amend, noVerify);
      showToast("success", t("commit.done", { hash: res.short_hash }));
      if (andPush) {
        // P6: 真实推送（凭据接管在 P7 接入；HTTPS 公仓 / 已存凭据场景可用）。
        const tab = repos.active;
        const branch = tab?.branch;
        if (tab && branch) {
          try {
            const remotes = await git.remotes(id);
            const remote = remotes[0]?.name;
            if (remote) {
              await runPush(id, remote, branch, { setUpstream: true });
            } else {
              showToast("warning", t("commit.pushNoRemote"));
            }
          } catch (e) {
            normalizeError(e);
          }
        }
      }
      await repos.refresh(id);
    } catch (e) {
      normalizeError(e);
    } finally {
      commitBusy = false;
    }
  }

  // ---- recovery dialog ----
  let recoveryOpen = $state(false);
  let recoveryEntries = $state<RecoveryEntry[]>([]);
  let recoveryBusyId = $state<string | null>(null);

  async function openRecovery(): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    try {
      recoveryEntries = await recovery.list(id);
      recoveryOpen = true;
    } catch (e) {
      normalizeError(e);
    }
  }

  async function restoreSnapshot(snapshotId: string): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    recoveryBusyId = snapshotId;
    try {
      await recovery.restore(id, snapshotId);
      await repos.refresh(id);
      recoveryOpen = false;
      showToast("success", t("workspace.restored"));
    } catch (e) {
      normalizeError(e);
    } finally {
      recoveryBusyId = null;
    }
  }

  async function deleteSnapshot(snapshotId: string): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    recoveryBusyId = snapshotId;
    try {
      await recovery.remove(id, snapshotId);
      recoveryEntries = recoveryEntries.filter((e) => e.id !== snapshotId);
    } catch (e) {
      normalizeError(e);
    } finally {
      recoveryBusyId = null;
    }
  }
</script>

<div class="flex min-h-0 flex-1 flex-col">
  {#if active?.operation}
    <div transition:slide={bannerCollapse}>
      <OperationBanner
        repoId={active.id}
        operation={active.operation}
        conflictCount={active.conflicts.length}
        onchanged={() => void repos.refresh(active.id)}
        onresolve={() => {
          const first = active.conflicts[0];
          if (first) repos.updateUi({ selected_file: { path: first.path, source: "worktree" } });
        }}
      />
    </div>
  {/if}
  <div class="flex min-h-0 flex-1">
  <!-- 文件列表 -->
  <div
    class="flex min-h-0 flex-col border-r {!fileListDragging
      ? 'transition-[width] duration-[120ms] ease-out'
      : ''}"
    style="width: {settings.fileListWidth}px"
  >
    <div class="flex items-center gap-2 p-2">
      <Input
        data-filter
        placeholder={t("workspace.filter")}
        bind:value={repos.ui.filter}
        class="h-8 bg-background text-[13px]"
      />
      <Button
        variant="ghost"
        size="icon-sm"
        title={repos.ui.view_mode === "list" ? t("workspace.treeView") : t("workspace.listView")}
        onclick={() => repos.updateUi({ view_mode: repos.ui.view_mode === "list" ? "tree" : "list" })}
      >
        {#if repos.ui.view_mode === "list"}
          <FolderTree class="size-4" />
        {:else}
          <List class="size-4" />
        {/if}
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        title={t("recovery.title")}
        onclick={openRecovery}
      >
        <History class="size-4" />
      </Button>
    </div>

    {#if !active}
      <div class="flex-1"></div>
    {:else if repos.ui.view_mode === "tree"}
      <StatusTree
        conflicts={sections.conflicts}
        staged={sections.staged}
        unstaged={sections.unstaged}
        activeKey={activeKey}
        filtered={filteredActive}
        {selection}
        collapsed={repos.ui.tree_collapsed}
        onleafclick={(file, source) =>
          repos.updateUi({ selected_file: { path: file.path, source } })}
        onleafcontext={onRowContext}
        onstage={doStage}
        onunstage={doUnstage}
        ondiscard={askDiscard}
      />
    {:else}
      <StatusSections
        conflicts={sections.conflicts}
        staged={sections.staged}
        unstaged={sections.unstaged}
        {activeKey}
        {conflictMeta}
        filtered={filteredActive}
        {selection}
        onrowclick={onRowClick}
        onrowcontext={onRowContext}
        onstage={doStage}
        onunstage={doUnstage}
        ondiscard={askDiscard}
      />
      {#if selectionCount > 1}
        <div class="flex items-center gap-2 border-t px-3 py-1.5 text-xs text-muted-foreground">
          <span>{t("workspace.selected", { n: selectionCount })}</span>
          <Button
            variant="ghost"
            size="xs"
            class="ml-auto"
            onclick={() => selection.clear()}
          >
            {t("workspace.clearSelection")}
          </Button>
        </div>
      {/if}
    {/if}
  </div>

  <PanelResizer
    bind:width={settings.fileListWidth}
    bind:dragging={fileListDragging}
    min={220}
    max={560}
    onCommit={(w) => void settings.setFileListWidth(w)}
  />

  <!-- 差异 + 提交框 -->
  <div class="flex min-w-0 flex-1 flex-col">
    <div class="min-h-0 flex-1">
      {#if selectedIsConflict && repos.ui.selected_file && active}
        <ConflictEditor
          repoId={active.id}
          path={repos.ui.selected_file.path}
          revision={active.lastRefreshMs ?? 0}
          onresolved={() => void repos.refresh(active.id)}
          onrefresh={() => void repos.refresh(active.id)}
        />
      {:else}
        <DiffViewer
          model={diffModel}
          loading={diffLoading}
          repoId={active?.id ?? null}
          ignoreWhitespace={diffIgnoreWs}
          onlineop={handleLineOp}
          onexpand={handleExpand}
          onignorewschange={handleIgnoreWs}
        />
      {/if}
    </div>

    {#if active?.operation?.kind !== "rebase" && active?.operation?.kind !== "apply"}
      <CommitBox
        bind:this={commitBox}
        {stagedCount}
        repoReady={active !== null}
        busy={commitBusy}
        mode={commitMode}
        template={templateForActive}
        oncommit={handleCommit}
        onamend={loadHeadMessage}
      />
    {/if}
  </div>
  </div>
</div>

<!-- 丢弃确认（快照兜底，可撤销） -->
<ConfirmDialog
  bind:open={discardOpen}
  title={t("workspace.discardConfirmTitle")}
  description={t("workspace.discardConfirmDesc", { n: discardPaths.length })}
  confirmLabel={t("workspace.discard")}
  destructive
  onconfirm={confirmDiscard}
>
  <div class="space-y-2 text-sm">
    <div class="max-h-32 overflow-y-auto rounded-md border p-2 font-mono text-xs text-muted-foreground">
      {#each discardPaths as p}
        <div class="truncate">{p}</div>
      {/each}
    </div>
    {#if discardHasUntracked}
      <div class="text-xs text-amber-600 dark:text-amber-500">{t("workspace.discardUntrackedWarn")}</div>
    {/if}
  </div>
</ConfirmDialog>

<!-- 忽略文件确认 -->
<ConfirmDialog
  bind:open={ignoreOpen}
  title={t("workspace.ignoreConfirmTitle")}
  description={t("workspace.ignoreConfirmDesc")}
  confirmLabel={t("workspace.ctx.ignore")}
  onconfirm={confirmIgnore}
>
  <div class="rounded-md border p-2 font-mono text-xs text-muted-foreground">
    {#each ignorePaths as p}
      <div class="truncate">{p}</div>
    {/each}
  </div>
</ConfirmDialog>

<!-- 恢复列表 -->
<RecoveryDialog
  bind:open={recoveryOpen}
  entries={recoveryEntries}
  busyId={recoveryBusyId}
  onrestore={restoreSnapshot}
  ondelete={deleteSnapshot}
/>

<FileContextMenu
  target={ctxTarget}
  onclose={() => (ctxTarget = null)}
  onhistory={openFileHistory}
  onblame={openBlame}
  onreveal={reveal}
  oncopypath={copyPath}
  onignore={askIgnore}
/>
