<script lang="ts">
  import { Tree, type TreeNode } from "$lib/components/ui/tree";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { git } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import Files from "@lucide/svelte/icons/files";
  import FileText from "@lucide/svelte/icons/file-text";
  import History from "@lucide/svelte/icons/history";
  import Tags from "@lucide/svelte/icons/tags";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import GitFork from "@lucide/svelte/icons/git-fork";
  import Tag from "@lucide/svelte/icons/tag";
  import Archive from "@lucide/svelte/icons/archive";

  let { resizing = false }: { resizing?: boolean } = $props();

  const ALL_GROUPS = ["workspace", "branches", "remote", "tags", "stash"];

  let activeNodeId = $state<string | null>(null);
  let checkoutTarget = $state<string | null>(null);
  let checkingOut = $state(false);

  const changed = $derived(repos.activeChanged);
  const branches = $derived(repos.active?.branches ?? []);
  const expanded = $derived.by(() => {
    const collapsed = new Set(repos.ui.sidebar_collapsed);
    return new Set(ALL_GROUPS.filter((id) => !collapsed.has(id)));
  });

  const nodes = $derived.by<TreeNode[]>(() => {
    const branchNodes: TreeNode[] = branches.map((b) => ({
      id: `branch:${b.name}`,
      label: b.name,
      icon: GitBranch,
      current: b.current,
      trailing:
        b.ahead || b.behind
          ? `${b.ahead ? `${b.ahead}↑` : ""}${b.behind ? `${b.behind}↓` : ""}`
          : undefined,
    }));
    return [
      {
        id: "workspace",
        label: t("sidebar.workspace"),
        icon: Files,
        children: [
          {
            id: "view:changes",
            payload: "changes",
            label: t("sidebar.changes"),
            icon: FileText,
            badge: changed,
          },
          { id: "view:history", payload: "history", label: t("sidebar.history"), icon: History },
          { id: "view:tags", payload: "tags", label: t("sidebar.tagsView"), icon: Tags },
        ],
      },
      {
        id: "branches",
        label: t("sidebar.branches"),
        icon: GitBranch,
        children:
          branchNodes.length > 0
            ? branchNodes
            : [{ id: "branches:empty", label: t("sidebar.noBranches"), muted: true }],
      },
      {
        id: "remote",
        label: t("sidebar.remote"),
        icon: GitFork,
        children: [
          { id: "remote:soon", label: t("sidebar.comingSoon", { phase: "P6" }), muted: true },
        ],
      },
      {
        id: "tags",
        label: t("sidebar.tags"),
        icon: Tag,
        children: [{ id: "tags:soon", label: t("sidebar.comingSoon", { phase: "P6" }), muted: true }],
      },
      {
        id: "stash",
        label: t("sidebar.stash"),
        icon: Archive,
        children: [
          { id: "stash:soon", label: t("sidebar.comingSoon", { phase: "P6" }), muted: true },
        ],
      },
    ];
  });

  function toggle(node: TreeNode, isOpen: boolean): void {
    const collapsed = new Set(repos.ui.sidebar_collapsed);
    if (isOpen) collapsed.delete(node.id);
    else collapsed.add(node.id);
    repos.updateUi({ sidebar_collapsed: [...collapsed] });
  }

  function activate(node: TreeNode): void {
    if (node.payload === "changes" || node.payload === "history" || node.payload === "tags") {
      repos.updateUi({ view: node.payload });
      activeNodeId = node.id;
    } else {
      activeNodeId = node.id;
    }
  }

  async function doCheckout(): Promise<void> {
    const name = checkoutTarget;
    const id = repos.activeId;
    if (!name || id === null) return;
    checkingOut = true;
    checkoutTarget = null;
    try {
      await git.checkoutBranch(id, name);
      await repos.refresh(id);
      showToast("success", t("sidebar.checkoutDone", { name }));
    } finally {
      checkingOut = false;
    }
  }
</script>

<aside
  class="flex min-h-0 shrink-0 flex-col overflow-hidden border-r bg-muted/30 {!resizing
    ? 'transition-[width] duration-[120ms] ease-out'
    : ''}"
  style="width: {settings.sidebarWidth}px"
>
  <div class="min-h-0 flex-1 overflow-y-auto p-1.5">
    <Tree
      {nodes}
      {expanded}
      activeId={activeNodeId}
      onToggle={toggle}
      onActivate={activate}
      onActivateSecondary={(node) => {
        if (node.id.startsWith("branch:") && !node.current) checkoutTarget = node.payload ?? null;
      }}
    />
  </div>
</aside>

<Dialog.Root
  open={checkoutTarget !== null}
  onOpenChange={(o) => !o && (checkoutTarget = null)}
>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>{t("dialog.checkout.title")}</Dialog.Title>
      <Dialog.Description>
        {t("sidebar.checkoutMsg", { name: checkoutTarget ?? "" })}
      </Dialog.Description>
    </Dialog.Header>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (checkoutTarget = null)}>
        {t("common.cancel")}
      </Button>
      <Button disabled={checkingOut} onclick={doCheckout}>{t("common.ok")}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
