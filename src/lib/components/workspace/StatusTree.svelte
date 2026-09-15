<script lang="ts">
  import { Tree } from "$lib/components/ui/tree";
  import type { TreeNode } from "$lib/components/ui/tree";
  import { t } from "$lib/i18n";
  import type { FileStatus } from "$lib/git";

  let {
    conflicts = [],
    staged = [],
    unstaged = [],
    activeKey = null,
    collapsed,
    onleafclick,
  }: {
    conflicts?: FileStatus[];
    staged?: FileStatus[];
    unstaged?: FileStatus[];
    /** Key of the active file (`source:path`). */
    activeKey?: string | null;
    /** Reactive array of collapsed directory ids (persisted per repo). */
    collapsed: string[];
    onleafclick: (file: FileStatus, source: "worktree" | "staged") => void;
  } = $props();

  interface FileLeaf {
    file: FileStatus;
    source: "worktree" | "staged";
  }

  let expanded = $state<Set<string>>(new Set());
  let allDirIds = $state<string[]>([]);

  // Rebuild the tree whenever the sections change.
  const nodes = $derived.by(() => {
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
        ensure("").leaves.push(leaf);
      }
    };

    for (const f of conflicts) add({ file: f, source: "worktree" });
    // A path both staged and unstaged renders once (worktree leaf wins).
    const unstagedSet = new Set(unstaged.map((f) => f.path));
    for (const f of staged)
      if (!unstagedSet.has(f.path)) add({ file: f, source: "staged" });
    for (const f of unstaged) add({ file: f, source: "worktree" });

    const ids: string[] = [];
    function build(prefix: string): TreeNode[] {
      const out: TreeNode[] = [];
      // Directories of this level, in name order.
      const level = [...dirs.keys()]
        .filter((p) => (prefix === "" ? !p.includes("/") : p.startsWith(`${prefix}/`)))
        .filter((p) => (prefix === "" ? true : p.slice(prefix.length + 1).split("/").length === 1))
        .sort();
      for (const path of level) {
        const node = dirs.get(path)!;
        ids.push(path);
        out.push({
          id: path,
          label: path.slice(prefix.length + 1),
          badge: node.count,
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
    const nodes = build("");
    allDirIds = ids;
    return nodes;
  });

  function statusLetter(file: FileStatus): string {
    if (file.conflict) return "!";
    if (file.untracked) return "U";
    const s = file.status;
    if (s.includes("D")) return "D";
    if (s.includes("R") || s.includes("C")) return "R";
    if (s.includes("A")) return "A";
    return "M";
  }

  // Expanded = every directory minus the persisted collapsed list.
  $effect(() => {
    expanded = new Set(allDirIds.filter((id) => !collapsed.includes(id)));
  });

  function toggle(node: TreeNode, open: boolean): void {
    const idx = collapsed.indexOf(node.id);
    if (open && idx !== -1) {
      collapsed.splice(idx, 1);
    } else if (!open && idx === -1) {
      collapsed.push(node.id);
    }
  }

  function activate(node: TreeNode): void {
    if (node.children?.length) return;
    const sep = node.id.indexOf(":");
    if (sep === -1) return;
    const source = node.id.slice(0, sep) === "staged" ? "staged" : "worktree";
    const path = node.id.slice(sep + 1);
    const pool = source === "staged" ? staged : [...conflicts, ...unstaged];
    const file = pool.find((f) => f.path === path);
    if (file) onleafclick(file, source);
  }

  const empty = $derived(conflicts.length === 0 && staged.length === 0 && unstaged.length === 0);
</script>

<div class="min-h-0 flex-1 overflow-y-auto pb-2">
  {#if empty}
    <div class="px-3 py-6 text-center text-xs text-muted-foreground">
      {t("workspace.empty")}
    </div>
  {:else}
    <Tree {nodes} {expanded} activeId={activeKey} onToggle={toggle} onActivate={activate} />
  {/if}
</div>
