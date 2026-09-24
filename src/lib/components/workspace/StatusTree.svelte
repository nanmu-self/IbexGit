<script lang="ts">
  import { Tree } from "$lib/components/ui/tree";
  import type { TreeNode } from "$lib/components/ui/tree";
  import { EmptyState } from "$lib/components/ui/empty-state";
  import { t } from "$lib/i18n";
  import type { FileStatus } from "$lib/git";
  import StatusSectionHeader from "./StatusSectionHeader.svelte";

  let {
    conflicts = [],
    staged = [],
    unstaged = [],
    activeKey = null,
    filtered = false,
    selection,
    collapsed,
    onleafclick,
    onleafcontext,
    onstage,
    onunstage,
    ondiscard,
  }: {
    conflicts?: FileStatus[];
    staged?: FileStatus[];
    unstaged?: FileStatus[];
    /** Key of the active file (`source:path`). */
    activeKey?: string | null;
    /** A filter is active: empty result gets a different message. */
    filtered?: boolean;
    /** Reactive SvelteSet of selected row keys (`source:path`), for batch labels. */
    selection: Set<string>;
    /** Reactive array of collapsed directory ids (persisted per repo). */
    collapsed: string[];
    onleafclick: (file: FileStatus, source: "worktree" | "staged", e?: MouseEvent) => void;
    /** Right-click on a leaf (list-view parity: context menu). */
    onleafcontext?: (
      file: FileStatus,
      source: "worktree" | "staged",
      e: MouseEvent,
    ) => void;
    onstage: (paths: string[]) => void;
    onunstage: (paths: string[]) => void;
    ondiscard: (paths: string[], scope: "worktree" | "all") => void;
  } = $props();

  interface FileLeaf {
    file: FileStatus;
    source: "worktree" | "staged";
  }

  function statusLetter(file: FileStatus): string {
    if (file.conflict) return "!";
    if (file.untracked) return "U";
    const s = file.status;
    if (s.includes("D")) return "D";
    if (s.includes("R") || s.includes("C")) return "R";
    if (s.includes("A")) return "A";
    return "M";
  }

  // Build one section's tree model. Pure — Svelte 5 forbids writing $state
  // inside $derived, so the dir-id list rides along in the return value
  // instead of being assigned to component state (state_unsafe_mutation).
  function buildModel(
    leaves: FileLeaf[],
    badgeTone: "red" | "muted" = "muted",
  ): { nodes: TreeNode[]; ids: string[] } {
    interface DirNode {
      dirs: Map<string, DirNode>;
      leaves: FileLeaf[];
      count: number;
    }
    const dirs = new Map<string, DirNode>();
    const ensure = (path: string): DirNode => {
      let node = dirs.get(path);
      if (!node) {
        node = { dirs: new Map<string, DirNode>(), leaves: [], count: 0 };
        dirs.set(path, node);
      }
      return node;
    };
    const add = (leaf: FileLeaf): void => {
      const parts = leaf.file.path.split("/");
      // Bump the change counter along the whole ancestor chain.
      for (let i = 1; i < parts.length; i++) {
        ensure(parts.slice(0, i).join("/")).count++;
      }
      if (parts.length > 1) {
        ensure(parts.slice(0, -1).join("/")).leaves.push(leaf);
      } else {
        // "" is the root bucket for top-level files' leaves, not a real
        // directory — `build("")` must never list it (infinite recursion).
        ensure("").leaves.push(leaf);
      }
    };
    for (const leaf of leaves) add(leaf);

    const ids: string[] = [];
    function build(prefix: string): TreeNode[] {
      const out: TreeNode[] = [];
      // Directories of this level, in name order ("" excluded, see above).
      const level = [...dirs.keys()]
        .filter((p) =>
          prefix === "" ? p !== "" && !p.includes("/") : p.startsWith(`${prefix}/`),
        )
        .filter((p) =>
          prefix === "" ? true : p.slice(prefix.length + 1).split("/").length === 1,
        )
        .sort();
      for (const path of level) {
        const node = dirs.get(path)!;
        ids.push(path);
        out.push({
          id: path,
          // Root level has no parent prefix to strip — slicing would chop
          // the first character ("src" → "rc").
          label: prefix === "" ? path : path.slice(prefix.length + 1),
          badge: node.count,
          badgeTone,
          children: build(path),
        });
      }
      // Files directly under `prefix` ("" = root).
      const node = dirs.get(prefix);
      if (node) {
        for (const leaf of node.leaves) {
          const name = leaf.file.path.split("/").pop() ?? leaf.file.path;
          out.push({
            id: `${leaf.source}:${leaf.file.path}`,
            label: name,
            badge: statusLetter(leaf.file),
            payload: leaf.source,
          });
        }
      }
      return out;
    }
    return { nodes: build(""), ids };
  }

  // One tree per section, mirroring the list view's information layout
  // (a path both staged and unstaged appears in both sections).
  const conflictModel = $derived(
    buildModel(conflicts.map((f) => ({ file: f, source: "worktree" as const })), "red"),
  );
  const stagedModel = $derived(
    buildModel(staged.map((f) => ({ file: f, source: "staged" as const }))),
  );
  const unstagedModel = $derived(
    buildModel(unstaged.map((f) => ({ file: f, source: "worktree" as const }))),
  );

  // Expanded = every directory minus the persisted collapsed list (shared
  // across sections: collapsing "src" hides it in every section's tree).
  const expanded = $derived(
    new Set(
      [...conflictModel.ids, ...stagedModel.ids, ...unstagedModel.ids].filter(
        (id) => !collapsed.includes(id),
      ),
    ),
  );

  function toggle(node: TreeNode, open: boolean): void {
    const idx = collapsed.indexOf(node.id);
    if (open && idx !== -1) {
      collapsed.splice(idx, 1);
    } else if (!open && idx === -1) {
      collapsed.push(node.id);
    }
  }

  function leafOf(node: TreeNode): FileLeaf | null {
    if (node.children?.length) return null;
    const sep = node.id.indexOf(":");
    if (sep === -1) return null;
    const source = node.id.slice(0, sep) === "staged" ? "staged" : "worktree";
    const path = node.id.slice(sep + 1);
    const pool = source === "staged" ? staged : [...conflicts, ...unstaged];
    const file = pool.find((f) => f.path === path);
    return file ? { file, source } : null;
  }

  function activate(node: TreeNode, e?: MouseEvent): void {
    const leaf = leafOf(node);
    if (leaf) onleafclick(leaf.file, leaf.source, e);
  }

  function leafContext(node: TreeNode, e: MouseEvent): void {
    if (!onleafcontext) return;
    const leaf = leafOf(node);
    if (leaf) onleafcontext(leaf.file, leaf.source, e);
  }

  // ---- selection-aware batch actions (list-view parity) ----
  function selectedIn(files: FileStatus[], source: "worktree" | "staged"): string[] {
    return files
      .filter((f) => selection.has(`${source}:${f.path}`))
      .map((f) => f.path);
  }

  const conflictActions = $derived.by(() => {
    if (conflicts.length === 0) return [];
    return [
      {
        label: t("workspace.discardAll"),
        danger: true,
        onclick: () => ondiscard(conflicts.map((f) => f.path), "all"),
      },
    ];
  });

  const stagedActions = $derived.by(() => {
    if (staged.length === 0) return [];
    const sel = selectedIn(staged, "staged");
    return [
      {
        label:
          sel.length > 0
            ? t("workspace.unstageSelected", { n: sel.length })
            : t("workspace.unstageAll"),
        onclick: () => onunstage(sel.length > 0 ? sel : staged.map((f) => f.path)),
      },
    ];
  });

  const unstagedActions = $derived.by(() => {
    if (unstaged.length === 0) return [];
    const sel = selectedIn(unstaged, "worktree");
    const paths = sel.length > 0 ? sel : unstaged.map((f) => f.path);
    return [
      {
        label:
          sel.length > 0
            ? t("workspace.discardSelected", { n: sel.length })
            : t("workspace.discardAll"),
        danger: true,
        onclick: () => ondiscard(paths, "worktree"),
      },
      {
        label:
          sel.length > 0
            ? t("workspace.stageSelected", { n: sel.length })
            : t("workspace.stageAll"),
        onclick: () => onstage(paths),
      },
    ];
  });

  const empty = $derived(
    conflicts.length === 0 && staged.length === 0 && unstaged.length === 0,
  );
</script>

<div class="min-h-0 flex-1 overflow-y-auto pb-2">
  {#if empty}
    <EmptyState
      title={filtered ? t("workspace.noMatch") : t("workspace.empty")}
      hint={filtered ? "" : t("workspace.emptyHint")}
      compact
    />
  {:else}
    {#if conflicts.length > 0}
      <StatusSectionHeader
        title={t("workspace.conflicts")}
        count={conflicts.length}
        tone="red"
        actions={conflictActions}
      />
      <Tree
        nodes={conflictModel.nodes}
        {expanded}
        activeId={activeKey}
        onToggle={toggle}
        onActivate={activate}
        onLeafContext={leafContext}
      />
    {/if}

    <StatusSectionHeader
      title={t("workspace.staged")}
      count={staged.length}
      actions={stagedActions}
    />
    {#if staged.length > 0}
      <Tree
        nodes={stagedModel.nodes}
        {expanded}
        activeId={activeKey}
        onToggle={toggle}
        onActivate={activate}
        onLeafContext={leafContext}
      />
    {/if}

    <StatusSectionHeader
      title={t("workspace.unstaged")}
      count={unstaged.length}
      actions={unstagedActions}
    />
    {#if unstaged.length > 0}
      <Tree
        nodes={unstagedModel.nodes}
        {expanded}
        activeId={activeKey}
        onToggle={toggle}
        onActivate={activate}
        onLeafContext={leafContext}
      />
    {/if}
  {/if}
</div>
