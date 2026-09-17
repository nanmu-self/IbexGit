<script lang="ts">
  // P5 提交详情面板: metadata + changed files (list/tree) + diff reuse
  // (DiffViewer, source=commit, read-only) + "restore this file version".
  import { fade } from "svelte/transition";
  import { Button } from "$lib/components/ui/button";
  import { ConfirmDialog } from "$lib/components/ui/confirm-dialog";
  import { Tree, type TreeNode } from "$lib/components/ui/tree";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { t } from "$lib/i18n";
  import {
    git,
    recovery,
    normalizeError,
    EMPTY_TREE_SHA,
    type CommitDetail,
    type DiffModel,
  } from "$lib/git";
  import { repos } from "$lib/stores/repos.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { showToast } from "$lib/stores/toast";
  import { requestRefAction } from "$lib/stores/refbus";
  import { fileView } from "$lib/stores/fileview.svelte";
  import List from "@lucide/svelte/icons/list";
  import FolderTree from "@lucide/svelte/icons/folder-tree";
  import Copy from "@lucide/svelte/icons/copy";
  import History from "@lucide/svelte/icons/history";
  import TextSelect from "@lucide/svelte/icons/text-select";
  import DiffFileIcon from "@lucide/svelte/icons/file-diff";
  import Tag from "@lucide/svelte/icons/tag";

  let {
    hash,
    onClose,
  }: {
    /** Commit under inspection; null hides the panel. */
    hash: string | null;
    onClose: () => void;
  } = $props();

  let detail = $state<CommitDetail | null>(null);
  let loading = $state(false);
  let viewMode = $state<"list" | "tree">("list");
  let selectedFile = $state<string | null>(null);
  let diffModel = $state<DiffModel | null>(null);
  let diffLoading = $state(false);
  let restoreOpen = $state(false);
  let restorePath = $state<string | null>(null);
  let treeCollapsed = $state<string[]>([]);

  const parent = $derived(detail?.parent ?? null);

  // ---- load detail whenever the selection changes ----
  $effect(() => {
    const id = repos.activeId;
    const h = hash;
    const refreshMark = repos.active?.lastRefreshMs;
    if (!id || !h) {
      detail = null;
      selectedFile = null;
      diffModel = null;
      return;
    }
    loading = true;
    selectedFile = null;
    diffModel = null;
    git
      .commitDetail(id, h)
      .then((d) => {
        if (hash === h && repos.activeId === id) detail = d;
      })
      .catch((e) => normalizeError(e))
      .finally(() => {
        if (hash === h) loading = false;
      });
    void refreshMark;
  });

  // ---- load the file diff (old_rev = parent or empty tree) ----
  $effect(() => {
    const id = repos.activeId;
    const h = hash;
    const file = selectedFile;
    const p = parent;
    if (!id || !h || !file) {
      diffModel = null;
      return;
    }
    diffLoading = true;
    const oldRev = p ?? EMPTY_TREE_SHA;
    git
      .diff(id, "commit", oldRev, h, [file])
      .then((m) => {
        if (selectedFile === file && hash === h) diffModel = m;
      })
      .catch((e) => {
        normalizeError(e);
        diffModel = null;
      })
      .finally(() => {
        if (selectedFile === file) diffLoading = false;
      });
  });

  // ---- changed-file tree nodes (viewMode = tree) ----
  const treeNodes = $derived.by<TreeNode[]>(() => {
    const files = detail?.files ?? [];
    if (files.length === 0) return [];

    interface Dir {
      dirs: Map<string, Dir>;
      leaves: CommitDetail["files"];
    }
    const root: Dir = { dirs: new Map(), leaves: [] };
    for (const f of files) {
      const parts = f.path.split("/");
      let cur = root;
      for (let i = 0; i < parts.length - 1; i++) {
        let next = cur.dirs.get(parts[i]);
        if (!next) {
          next = { dirs: new Map(), leaves: [] };
          cur.dirs.set(parts[i], next);
        }
        cur = next;
      }
      cur.leaves.push(f);
    }
    const toNodes = (dir: Dir): TreeNode[] => {
      const nodes: TreeNode[] = [];
      for (const [name, sub] of dir.dirs) {
        nodes.push({
          id: `dir:${name}:${sub.leaves.length}`,
          label: name,
          badge: count(sub),
          children: toNodes(sub),
        });
      }
      for (const f of dir.leaves) {
        nodes.push({
          id: `file:${f.path}`,
          label: f.path.split("/").pop() ?? f.path,
          payload: f.path,
          trailing: f.status,
        });
      }
      return nodes;
    };
    const count = (dir: Dir): number =>
      dir.leaves.length +
      [...dir.dirs.values()].reduce((n, d) => n + count(d), 0);
    return toNodes(root);
  });

  const treeExpanded = $derived.by(() => {
    // All directories expanded except those the user collapsed.
    const all = new Set<string>();
    const walk = (nodes: TreeNode[]): void => {
      for (const n of nodes) {
        if (n.children) {
          all.add(n.id);
          walk(n.children);
        }
      }
    };
    walk(treeNodes);
    for (const id of treeCollapsed) all.delete(id);
    return all;
  });

  function statusColor(status: string): string {
    switch (status) {
      case "A":
        return "text-emerald-600 dark:text-emerald-400";
      case "D":
        return "text-red-600 dark:text-red-400";
      case "R":
      case "C":
        return "text-violet-600 dark:text-violet-400";
      case "U":
        return "text-orange-600 dark:text-orange-400";
      default:
        return "text-amber-600 dark:text-amber-400";
    }
  }

  function copyHash(): void {
    if (!detail) return;
    navigator.clipboard?.writeText(detail.commit.hash).catch(() => {});
  }

  // ---- restore this file version (snapshot-backed, undoable) ----
  function askRestore(path: string): void {
    restorePath = path;
    restoreOpen = true;
  }

  // ---- file row context menu (P9 历史右键入口) ----
  let fileMenu = $state<{ path: string; x: number; y: number } | null>(null);

  function openFileMenu(path: string, e: MouseEvent): void {
    e.preventDefault();
    e.stopPropagation();
    fileMenu = { path, x: e.clientX, y: e.clientY };
  }

  $effect(() => {
    if (!fileMenu) return;
    const ondown = (e: MouseEvent): void => {
      const panel = document.getElementById("file-row-menu");
      if (!panel || !panel.contains(e.target as Node)) fileMenu = null;
    };
    const onescape = (e: KeyboardEvent): void => {
      if (e.key === "Escape") fileMenu = null;
    };
    window.addEventListener("mousedown", ondown, true);
    window.addEventListener("keydown", onescape, true);
    return () => {
      window.removeEventListener("mousedown", ondown, true);
      window.removeEventListener("keydown", onescape, true);
    };
  });

  function fileMenuStyle(x: number, y: number): string {
    const W = 200;
    const H = 92;
    return `left:${Math.min(x, window.innerWidth - W - 8)}px;top:${Math.min(y, window.innerHeight - H - 8)}px`;
  }

  async function confirmRestore(): Promise<void> {
    const id = repos.activeId;
    const h = hash;
    if (!id || !h || !restorePath) return;
    try {
      const snapId = await git.restoreFileVersion(id, h, [restorePath]);
      showToast(
        "success",
        t("history.restoreDone", { path: restorePath }),
        t("workspace.discardUndoHint"),
        8000,
        snapId
          ? {
              label: t("workspace.undo"),
              run: () => {
                recovery.restore(id, snapId).catch((e) => normalizeError(e));
              },
            }
          : undefined,
      );
      await repos.refresh(id);
    } catch (e) {
      normalizeError(e);
    }
  }
</script>

<aside
  class="flex min-h-0 flex-col border-l bg-background"
  style="width: {settings.historyDetailWidth}px"
>
  {#if !hash}
    <div
      class="flex flex-1 items-center justify-center p-4 text-center text-sm text-muted-foreground"
    >
      {t("history.selectCommit")}
    </div>
  {:else if loading && !detail}
    <div
      class="flex flex-1 items-center justify-center text-sm text-muted-foreground"
    >
      {t("common.loading")}
    </div>
  {:else if detail}
    <!-- metadata -->
    <div class="border-b p-3">
      <div class="flex items-start justify-between gap-2">
        <h3 class="min-w-0 flex-1 text-sm leading-snug font-medium break-words">
          {detail.commit.message}
        </h3>
        <div class="flex shrink-0 items-center gap-0.5">
          <Button
            variant="ghost"
            size="icon-sm"
            title={t("refs.tagDialog.title")}
            onclick={() => requestRefAction({ kind: "newTag", target: hash })}
          >
            <Tag class="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            title={t("history.copyHash")}
            onclick={copyHash}
          >
            <Copy class="size-3.5" />
          </Button>
          <Button variant="ghost" size="icon-sm" title={t("common.close")} onclick={onClose}>
            ✕
          </Button>
        </div>
      </div>
      <div
        class="mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-muted-foreground"
      >
        <span>{detail.commit.author}</span>
        <span>{new Date(detail.commit.date).toLocaleString()}</span>
        <span class="font-mono">{detail.commit.short_hash}</span>
      </div>
    </div>

    <!-- files -->
    <div class="flex items-center gap-2 border-b px-3 py-1.5">
      <span class="text-xs text-muted-foreground">
        {t("history.changedFiles", { n: detail.files.length })}
      </span>
      <Button
        variant="ghost"
        size="icon-sm"
        class="ml-auto"
        title={viewMode === "list" ? t("workspace.treeView") : t("workspace.listView")}
        onclick={() => (viewMode = viewMode === "list" ? "tree" : "list")}
      >
        {#if viewMode === "list"}
          <FolderTree class="size-3.5" />
        {:else}
          <List class="size-3.5" />
        {/if}
      </Button>
    </div>

    <div class="max-h-[38%] min-h-[80px] overflow-y-auto border-b">
      {#if viewMode === "list"}
        {#each detail.files as f (f.path)}
          <div
            class="group flex cursor-pointer items-center gap-2 px-3 py-1 text-xs hover:bg-muted/60 {selectedFile === f.path ? 'bg-muted' : ''}"
            onclick={() => (selectedFile = f.path)}
            role="button"
            tabindex="0"
            onkeydown={(e) => e.key === "Enter" && (selectedFile = f.path)}
            oncontextmenu={(e) => openFileMenu(f.path, e)}
          >
            <span
              class="w-4 shrink-0 text-center font-mono font-semibold {statusColor(f.status)}"
            >
              {f.status}
            </span>
            <span class="min-w-0 flex-1 truncate font-mono" title={f.path}>
              {#if f.orig_path}
                <span class="text-muted-foreground line-through">{f.orig_path}</span>
                → {f.path}
              {:else}
                {f.path}
              {/if}
            </span>
            <button
              class="hidden shrink-0 rounded p-0.5 hover:bg-accent group-hover:block"
              title={t("history.restoreFile")}
              onclick={(e) => {
                e.stopPropagation();
                askRestore(f.path);
              }}
            >
              <History class="size-3.5" />
            </button>
          </div>
        {/each}
      {:else}
        <Tree
          nodes={treeNodes}
          expanded={treeExpanded}
          activeId={selectedFile ? `file:${selectedFile}` : null}
          onToggle={(node, open) => {
            treeCollapsed = open
              ? treeCollapsed.filter((id) => id !== node.id)
              : [...treeCollapsed, node.id];
          }}
          onActivate={(node) => {
            if (node.payload) selectedFile = node.payload;
          }}
        />
      {/if}
    </div>

    <!-- diff -->
    <div class="min-h-0 flex-1">
      {#if selectedFile}
        <DiffViewer
          model={diffModel}
          loading={diffLoading}
          repoId={repos.activeId}
          ignoreWhitespace={false}
          onlineop={() => {}}
          onexpand={() => {}}
          onignorewschange={() => {}}
        />
      {:else}
        <div
          class="flex h-full items-center justify-center p-4 text-center text-sm text-muted-foreground"
        >
          <div>
            <DiffFileIcon class="mx-auto mb-2 size-6 opacity-40" />
            {t("history.selectFile")}
          </div>
        </div>
      {/if}
    </div>
  {/if}
</aside>

<ConfirmDialog
  bind:open={restoreOpen}
  title={t("history.restoreConfirmTitle")}
  description={t("history.restoreConfirmDesc", { path: restorePath ?? "" })}
  confirmLabel={t("history.restoreFile")}
  onconfirm={confirmRestore}
/>

<!-- 文件行右键菜单（P9：查看文件历史 / Blame） -->
{#if fileMenu}
  <div
    id="file-row-menu"
    class="fixed z-50 min-w-48 rounded-md border bg-popover p-1 text-popover-foreground shadow-md"
    transition:fade={{ duration: 100 }}
    style={fileMenuStyle(fileMenu.x, fileMenu.y)}
  >
    <button
      type="button"
      class="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-[13px] hover:bg-accent"
      onclick={() => {
        fileView.show(fileMenu!.path, "history");
        fileMenu = null;
      }}
    >
      <History class="size-3.5" /> {t("history.ctx.fileHistory")}
    </button>
    <button
      type="button"
      class="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-[13px] hover:bg-accent"
      onclick={() => {
        fileView.show(fileMenu!.path, "blame");
        fileMenu = null;
      }}
    >
      <TextSelect class="size-3.5" /> {t("history.ctx.fileBlame")}
    </button>
  </div>
{/if}
