<script lang="ts">
  // Merge / Rebase dialog (P6 + MR 预览)：merge 模式 = 合并请求风格的富预览
  //（来源/目标及各自 tip 提交、提交列表、文件改动树 + 单文件 diff，diff =
  // merge_base…来源 的三点 diff）；rebase 模式保持紧凑表单。合并执行走
  // 宿主 onexecute（含 P8 冲突横幅接管 + 刷新），组件只负责预览与选参。
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import SegmentedControl from "$lib/components/ui/segmented-control/SegmentedControl.svelte";
  import { Tree, type TreeNode } from "$lib/components/ui/tree";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { t } from "$lib/i18n";
  import { git, normalizeError, type CommitInfo, type DiffModel, type FileStat } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Copy from "@lucide/svelte/icons/copy";
  import FileDiffIcon from "@lucide/svelte/icons/file-diff";
  import ArrowDownFromLine from "@lucide/svelte/icons/arrow-down-from-line";
  import Binary from "@lucide/svelte/icons/binary";

  // 不相关历史时的 diff 基准：空树（SHA-1 仓库；SHA-256 场景 diff 会报错，
  // 由错误提示兜底——本地 init 默认 SHA-1，远端托管平台同理）。
  const EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

  let {
    open = $bindable(false),
    mode = $bindable<"merge" | "rebase">("merge"),
    branches = [],
    currentBranch = "",
    initialTarget = "",
    repoId,
    busy = false,
    onexecute,
  }: {
    open?: boolean;
    mode?: "merge" | "rebase";
    branches?: { name: string }[];
    currentBranch?: string;
    initialTarget?: string;
    repoId: string | null;
    busy?: boolean;
    /** Returns a promise; the dialog closes only on success. */
    onexecute: (
      mode: "merge" | "rebase",
      target: string,
      strategy: "merge_node" | "squash" | "rebase" | "ff_only",
      allowUnrelatedHistories: boolean
    ) => Promise<void>;
  } = $props();

  let target = $state("");
  /** 合并方式：默认（总是合并节点）/ Squash / Rebase / FF-only。 */
  let strategy = $state<"merge_node" | "squash" | "rebase" | "ff_only">("merge_node");

  // ===================== merge 模式（MR 风格预览） =====================
  let tab = $state<"commits" | "files">("commits");
  /** merge_base 为 null（远端由网页端初始化等）：必须带 --allow-unrelated-histories。 */
  let unrelated = $state(false);
  let preview = $state<CommitInfo[] | null>(null);
  let previewLoading = $state(false);
  /** 来源/目标分支的 tip 提交（作者、说明、时间、短哈希）。 */
  let tipSource = $state<CommitInfo | null>(null);
  let tipTarget = $state<CommitInfo | null>(null);
  /** 三点 diff 的基准：merge_base（不相关历史 → 空树）。 */
  let diffBase = $state(EMPTY_TREE);
  let fileStats = $state<FileStat[]>([]);
  let selectedFile = $state<string | null>(null);
  let fileModel = $state<DiffModel | null>(null);
  let fileLoading = $state(false);
  /** onexpand 的上下文行数记忆（路径 → --unified=N）。 */
  let fileContext = $state(new Map<string, number>());

  /** Monotonic token — 只允许最新一次加载写结果（参照 CompareDialog）。 */
  let loadSeq = 0;
  let fileSeq = 0;
  let treeCollapsed = $state<string[]>([]);

  $effect(() => {
    if (open) {
      target = initialTarget;
      strategy = "merge_node";
      unrelated = false;
      preview = null;
      previewLoading = false;
      tipSource = null;
      tipTarget = null;
      fileStats = [];
      selectedFile = null;
      fileModel = null;
      fileContext = new Map();
      tab = "commits";
      treeCollapsed = [];
      loadSeq = 0;
      fileSeq = 0;
    }
  });

  // Dry-run 预览 + tip 提交 + 文件改动统计：来源分支变化时一次性加载
  //（全部读操作，seq token 丢弃过期结果）。rebase 模式只拉 revList 预览。
  $effect(() => {
    const tgt = target.trim();
    if (!open || !tgt || tgt === currentBranch) {
      preview = null;
      previewLoading = false;
      unrelated = false;
      tipSource = null;
      tipTarget = null;
      fileStats = [];
      selectedFile = null;
      fileModel = null;
      return;
    }
    previewLoading = true;
    const seq = ++loadSeq;
    const range = mode === "merge" ? `HEAD..${tgt}` : `${tgt}..HEAD`;
    const id = repoId ?? "";
    const basePromise =
      mode === "merge" && currentBranch
        ? git.branchCompare(id, currentBranch, tgt)
        : Promise.resolve(null);
    void Promise.all([
      git.revList(id, range, 50, 0).catch(() => null),
      basePromise.catch(() => null),
      mode === "merge" ? git.revList(id, tgt, 1, 0).catch(() => null) : Promise.resolve(null),
      mode === "merge" && currentBranch
        ? git.revList(id, currentBranch, 1, 0).catch(() => null)
        : Promise.resolve(null),
    ])
      .then(([commits, cmp, srcTip, dstTip]) => {
        if (seq !== loadSeq) return;
        preview = commits;
        const base = (cmp as { merge_base: string | null } | null)?.merge_base ?? null;
        unrelated = mode === "merge" && base === null;
        diffBase = base ?? EMPTY_TREE;
        tipSource = (srcTip as CommitInfo[] | null)?.[0] ?? null;
        tipTarget = (dstTip as CommitInfo[] | null)?.[0] ?? null;
        if (mode === "merge") {
          // 文件改动清单（空树基准 → 全部文件按 A 展示），默认选中第一个。
          git
            .diffStat(id, diffBase, tgt)
            .then((stats) => {
              if (seq !== loadSeq) return;
              fileStats = stats;
              selectedFile = stats[0]?.path ?? null;
            })
            .catch(() => {
              if (seq === loadSeq) fileStats = [];
            });
        }
      })
      .finally(() => {
        if (seq === loadSeq) previewLoading = false;
      });
  });

  // 选中文件 → 单文件 diff；上下文展开记忆在 fileContext（workspace
  // handleExpand 同语义），Map 换引用触发重拉。
  $effect(() => {
    const f = selectedFile;
    const id = repoId;
    const tgt = target.trim();
    if (!open || !f || !id || mode !== "merge" || !tgt || tgt === currentBranch) {
      fileModel = null;
      return;
    }
    const seq = ++fileSeq;
    fileLoading = true;
    const context = fileContext.get(f) ?? 3;
    git
      .diff(id, "commit", diffBase, tgt, [f], context)
      .then((m) => {
        if (seq === fileSeq) fileModel = m;
      })
      .catch((e) => {
        if (seq === fileSeq) normalizeError(e);
      })
      .finally(() => {
        if (seq === fileSeq) fileLoading = false;
      });
  });

  function handleExpand(path: string, dir: "up" | "down" | "all"): void {
    const current = fileContext.get(path) ?? 3;
    if (dir === "all") fileContext.set(path, 100_000);
    else fileContext.set(path, Math.min(current + 10, 100_000));
    fileContext = new Map(fileContext);
  }

  // ---- 文件改动树（目录分组 + 目录内文件计数，参照 CommitDetailPanel） ----
  const treeNodes = $derived.by<TreeNode[]>(() => {
    if (fileStats.length === 0) return [];
    interface Dir {
      dirs: Map<string, Dir>;
      leaves: FileStat[];
    }
    const root: Dir = { dirs: new Map(), leaves: [] };
    for (const f of fileStats) {
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
    const count = (dir: Dir): number =>
      dir.leaves.length + [...dir.dirs.values()].reduce((n, d) => n + count(d), 0);
    const toNodes = (dir: Dir): TreeNode[] => {
      const nodes: TreeNode[] = [];
      for (const [name, sub] of dir.dirs) {
        nodes.push({ id: `dir:${name}`, label: name, badge: count(sub), children: toNodes(sub) });
      }
      for (const f of dir.leaves) {
        nodes.push({
          id: `file:${f.path}`,
          label: f.path.split("/").pop() ?? f.path,
          payload: f.path,
          // 二进制文件树内以 B 标记（± 行数不可得）。
          trailing: f.additions === null ? "B" : f.status,
        });
      }
      return nodes;
    };
    return toNodes(root);
  });

  const treeExpanded = $derived.by(() => {
    // 全部展开，仅用户手动折叠的除外。
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

  const totalAdd = $derived(fileStats.reduce((n, f) => n + (f.additions ?? 0), 0));
  const totalDel = $derived(fileStats.reduce((n, f) => n + (f.deletions ?? 0), 0));
  const binaryCount = $derived(fileStats.filter((f) => f.additions === null).length);
  const selectedStat = $derived(fileStats.find((f) => f.path === selectedFile) ?? null);

  function statusColor(status: string): string {
    switch (status) {
      case "A":
        return "text-success dark:text-success";
      case "D":
        return "text-danger dark:text-danger";
      case "R":
      case "C":
        return "text-info dark:text-info";
      default:
        return "text-warning dark:text-warning";
    }
  }

  /** ISO 时间 → `YYYY-MM-DD HH:mm`。 */
  function shortDate(date: string): string {
    return date.slice(0, 16).replace("T", " ");
  }

  async function copyHash(hash: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(hash);
      showToast("info", t("history.copyHash"));
    } catch {
      /* clipboard unavailable */
    }
  }

  const STRATEGIES = [
    { id: "merge_node", label: "refs.merge.modeDefault", desc: "refs.merge.modeDefaultDesc" },
    { id: "squash", label: "refs.merge.modeSquash", desc: "refs.merge.modeSquashDesc" },
    { id: "rebase", label: "refs.merge.modeRebase", desc: "refs.merge.modeRebaseDesc" },
    { id: "ff_only", label: "refs.merge.modeFfOnly", desc: "refs.merge.modeFfOnlyDesc" },
  ] as const;
  const strategyDesc = $derived(
    t(STRATEGIES.find((s) => s.id === strategy)?.desc ?? "refs.merge.modeDefaultDesc")
  );
  /** rebase 合并需要共同祖先，不相关历史下不可用。 */
  const rebaseUnavailable = $derived(strategy === "rebase" && unrelated);

  async function executeMerge(): Promise<void> {
    try {
      // 宿主负责 git.merge + P8 冲突横幅接管 + toast + 刷新；失败保持打开。
      await onexecute("merge", target.trim(), strategy, unrelated);
      open = false;
    } catch (e) {
      normalizeError(e);
    }
  }

  // ===================== rebase 模式（原紧凑表单） =====================
  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (mode !== "rebase") return;
    try {
      await onexecute(mode, target.trim(), "merge_node", false);
      open = false;
    } catch (e) {
      normalizeError(e);
    }
  }
</script>

<Dialog.Root bind:open>
  {#if mode === "merge"}
    <!-- ============ MR 风格富预览 ============ -->
    <Dialog.Content
      class="flex h-[88vh] max-w-[calc(100vw-2rem)] sm:max-w-6xl flex-col gap-0 overflow-hidden p-0"
    >
      <div class="flex items-center gap-2 border-b py-2.5 pr-12 pl-4">
        <div class="flex min-w-0 flex-1 flex-col">
          <Dialog.Title class="text-sm font-medium">{t("refs.merge.title")}</Dialog.Title>
          <Dialog.Description class="text-xs text-muted-foreground">
            {t("refs.merge.desc", { branch: currentBranch })}
          </Dialog.Description>
        </div>
      </div>

      <!-- 来源 / 目标 -->
      <div class="space-y-1.5 border-b px-4 py-2.5">
        <div class="flex items-center gap-2">
          <span
            class="inline-flex shrink-0 items-center gap-1 rounded-md bg-muted px-1.5 py-0.5 text-xs text-muted-foreground"
          >
            <ArrowDownFromLine class="size-3" />
            {t("refs.merge.source")}
          </span>
          <select
            bind:value={target}
            class="h-7 w-44 shrink-0 rounded-md border bg-background px-1.5 font-mono text-[13px]"
          >
            <option value="" disabled>{t("refs.merge.pickTarget")}</option>
            {#each branches.filter((b) => b.name !== currentBranch) as b (b.name)}
              <option value={b.name}>{b.name}</option>
            {/each}
          </select>
          {#if tipSource}
            <span
              class="flex size-5 shrink-0 items-center justify-center rounded-full bg-primary/15 text-[10px] font-semibold text-primary"
              title={tipSource.author}
            >
              {tipSource.author.slice(0, 1).toUpperCase()}
            </span>
            <span class="min-w-0 truncate text-xs" title={tipSource.message}>{tipSource.message}</span>
            <span class="ml-auto shrink-0 text-xs text-muted-foreground">
              {shortDate(tipSource.date)}
            </span>
            <button
              type="button"
              class="flex shrink-0 items-center gap-0.5 rounded px-1 py-0.5 font-mono text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
              onclick={() => copyHash(tipSource?.hash ?? "")}
              title={tipSource.hash}
            >
              {tipSource.short_hash}
              <Copy class="size-3" />
            </button>
          {/if}
        </div>
        <div class="flex items-center gap-2">
          <span
            class="inline-flex shrink-0 items-center gap-1 rounded-md bg-muted px-1.5 py-0.5 text-xs text-muted-foreground"
          >
            <ArrowDownFromLine class="size-3 rotate-180" />
            {t("refs.merge.target")}
          </span>
          <span
            class="h-7 w-44 shrink-0 rounded-md border border-dashed bg-muted/40 px-1.5 font-mono text-[13px] leading-7"
          >
            {currentBranch || "—"}
          </span>
          {#if tipTarget}
            <span
              class="flex size-5 shrink-0 items-center justify-center rounded-full bg-primary/15 text-[10px] font-semibold text-primary"
              title={tipTarget.author}
            >
              {tipTarget.author.slice(0, 1).toUpperCase()}
            </span>
            <span class="min-w-0 truncate text-xs" title={tipTarget.message}>{tipTarget.message}</span>
            <span class="ml-auto shrink-0 text-xs text-muted-foreground">
              {shortDate(tipTarget.date)}
            </span>
            <button
              type="button"
              class="flex shrink-0 items-center gap-0.5 rounded px-1 py-0.5 font-mono text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
              onclick={() => copyHash(tipTarget?.hash ?? "")}
              title={tipTarget.hash}
            >
              {tipTarget.short_hash}
              <Copy class="size-3" />
            </button>
          {/if}
        </div>
      </div>

      <!-- tabs + 汇总 -->
      <div class="flex items-center justify-between gap-3 border-b px-4 py-2">
        <SegmentedControl
          bind:value={tab}
          options={[
            { value: "commits", label: t("refs.merge.tabCommits", { n: preview?.length ?? 0 }) },
            { value: "files", label: t("refs.merge.tabFiles", { n: fileStats.length }) },
          ]}
        />
        {#if tab === "files" && fileStats.length > 0}
          <div class="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
            <span class="shrink-0">{t("refs.merge.fileTotal", { n: fileStats.length })}</span>
            <span class="shrink-0 font-mono text-success dark:text-success">+{totalAdd}</span>
            <span class="shrink-0 font-mono text-danger dark:text-danger">−{totalDel}</span>
            {#if binaryCount > 0}
              <span class="inline-flex shrink-0 items-center gap-0.5">
                <Binary class="size-3" />
                {binaryCount}
              </span>
            {/if}
          </div>
        {/if}
      </div>

      <!-- body -->
      {#if previewLoading}
        <div class="flex flex-1 items-center justify-center gap-2 text-sm text-muted-foreground">
          <LoaderCircle class="size-4 animate-spin" />
          {t("common.loading")}
        </div>
      {:else if tab === "commits"}
        <div class="min-h-0 flex-1 overflow-y-auto">
          {#if preview && preview.length > 0}
            <ul class="divide-y">
              {#each preview as c (c.hash)}
                <li class="flex items-center gap-2 px-4 py-1.5 text-[13px]">
                  <span
                    class="flex size-5 shrink-0 items-center justify-center rounded-full bg-primary/15 text-[10px] font-semibold text-primary"
                  >
                    {c.author.slice(0, 1).toUpperCase()}
                  </span>
                  <span class="min-w-0 flex-1 truncate" title={c.message}>{c.message}</span>
                  <span class="shrink-0 text-xs text-muted-foreground">{c.author}</span>
                  <span class="shrink-0 text-xs text-muted-foreground">{shortDate(c.date)}</span>
                  <button
                    type="button"
                    class="shrink-0 rounded px-1 font-mono text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
                    onclick={() => copyHash(c.hash)}
                  >
                    {c.short_hash}
                  </button>
                </li>
              {/each}
            </ul>
          {:else}
            <div class="flex h-full items-center justify-center text-sm text-muted-foreground">
              {t("refs.merge.nothing")}
            </div>
          {/if}
        </div>
      {:else}
        <div class="flex min-h-0 flex-1">
          <!-- 文件树 -->
          <div class="w-60 shrink-0 overflow-y-auto border-r py-1">
            {#if treeNodes.length > 0}
              <Tree
                nodes={treeNodes}
                expanded={treeExpanded}
                activeId={selectedFile ? `file:${selectedFile}` : null}
                onToggle={(node, openSub) => {
                  treeCollapsed = openSub
                    ? treeCollapsed.filter((id) => id !== node.id)
                    : [...treeCollapsed, node.id];
                }}
                onActivate={(node) => {
                  if (node.payload) selectedFile = node.payload;
                }}
              />
            {:else}
              <p class="px-3 py-2 text-xs text-muted-foreground">{t("refs.merge.nothing")}</p>
            {/if}
          </div>
          <!-- diff -->
          <div class="flex min-w-0 flex-1 flex-col">
            {#if selectedFile && selectedStat}
              <div class="flex min-w-0 items-center gap-2 border-b px-3 py-1.5">
                <span
                  class="shrink-0 rounded px-1 font-mono text-xs font-semibold {statusColor(selectedStat.status)}"
                >
                  {selectedStat.status}{selectedStat.similarity ?? ""}
                </span>
                <span class="truncate font-mono text-xs" title={selectedFile}>{selectedFile}</span>
                {#if selectedStat.additions !== null}
                  <span class="ml-auto shrink-0 font-mono text-xs">
                    <span class="text-success dark:text-success">+{selectedStat.additions}</span>
                    <span class="ml-1.5 text-danger dark:text-danger">−{selectedStat.deletions}</span>
                  </span>
                {:else}
                  <span class="ml-auto shrink-0 text-xs text-muted-foreground">
                    {t("refs.merge.binaryFile")}
                  </span>
                {/if}
              </div>
              <DiffViewer
                model={fileModel}
                loading={fileLoading}
                repoId={repoId}
                ignoreWhitespace={false}
                onlineop={() => {}}
                onexpand={handleExpand}
                onignorewschange={() => {}}
              />
            {:else}
              <div
                class="flex h-full items-center justify-center p-4 text-center text-sm text-muted-foreground"
              >
                <div>
                  <FileDiffIcon class="mx-auto mb-2 size-6 opacity-40" />
                  {t("refs.merge.pickFile")}
                </div>
              </div>
            {/if}
          </div>
        </div>
      {/if}

      <!-- footer -->
      <div class="flex items-center gap-3 border-t px-4 py-2.5">
        <div class="min-w-0 flex-1 space-y-1.5">
          {#if unrelated}
            <p
              class="rounded-md bg-warning-surface px-2.5 py-1.5 text-xs text-warning dark:text-warning"
            >
              {t("refs.merge.unrelated")}
            </p>
          {/if}
          <div>
            <SegmentedControl
              bind:value={strategy}
              options={STRATEGIES.map((s) => ({ value: s.id, label: t(s.label) }))}
            />
            <p
              class="mt-1 text-xs {rebaseUnavailable
                ? 'text-warning dark:text-warning'
                : 'text-muted-foreground'}"
            >
              {rebaseUnavailable ? t("refs.merge.rebaseUnrelated") : strategyDesc}
            </p>
          </div>
        </div>
        <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
          {t("common.cancel")}
        </Button>
        <Button
          type="button"
          size="sm"
          disabled={busy || !target || rebaseUnavailable}
          onclick={() => void executeMerge()}
        >
          {#if busy}
            <LoaderCircle class="size-3.5 animate-spin" />
          {/if}
          {t("refs.merge.confirm")}
        </Button>
      </div>
    </Dialog.Content>
  {:else}
    <!-- ============ rebase 紧凑表单（保持原样） ============ -->
    <Dialog.Content class="max-w-md">
      <Dialog.Header>
        <Dialog.Title>{t("refs.rebase.title")}</Dialog.Title>
        <Dialog.Description>{t("refs.rebase.desc", { branch: currentBranch })}</Dialog.Description>
      </Dialog.Header>
      <form class="space-y-3" onsubmit={submit}>
        <select
          bind:value={target}
          class="h-9 w-full rounded-md border bg-background px-2 font-mono text-[13px]"
        >
          <option value="" disabled>{t("refs.merge.pickTarget")}</option>
          {#each branches.filter((b) => b.name !== currentBranch) as b (b.name)}
            <option value={b.name}>{b.name}</option>
          {/each}
        </select>

        {#if previewLoading}
          <div class="flex items-center gap-2 text-xs text-muted-foreground">
            <LoaderCircle class="size-3.5 animate-spin" />
            {t("common.loading")}
          </div>
        {:else if preview && preview.length > 0}
          <div>
            <div class="mb-1 text-xs text-muted-foreground">
              {t("refs.rebase.preview", { n: preview.length })}
            </div>
            <ul class="max-h-32 overflow-y-auto rounded-md border text-xs">
              {#each preview as c (c.hash)}
                <li class="flex items-center gap-2 border-b px-2 py-1 last:border-b-0">
                  <span class="font-mono text-muted-foreground">{c.short_hash}</span>
                  <span class="truncate">{c.message}</span>
                </li>
              {/each}
            </ul>
          </div>
        {:else if preview}
          <p class="text-xs text-muted-foreground">{t("refs.merge.nothing")}</p>
        {/if}

        <p class="rounded-md bg-warning-surface px-2.5 py-2 text-xs text-warning dark:text-warning">
          {t("refs.rebase.warn")}
        </p>

        <Dialog.Footer>
          <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
            {t("common.cancel")}
          </Button>
          <Button type="submit" size="sm" disabled={busy || !target} variant="destructive">
            {t("refs.rebase.confirm")}
          </Button>
        </Dialog.Footer>
      </form>
    </Dialog.Content>
  {/if}
</Dialog.Root>
