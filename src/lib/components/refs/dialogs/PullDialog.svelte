<script lang="ts">
  // Pull dialog (P6): remote / branch (default = upstream) + strategy
  // selection (merge / rebase / ff-only), per PLAN 合并/变基入口.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { t } from "$lib/i18n";
  import { git } from "$lib/git";
  import type { BranchInfo } from "$lib/git/bindings";
  import { settings } from "$lib/stores/settings.svelte";

  let {
    open = $bindable(false),
    repoId = null,
    remotes = [],
    branches = [],
    currentBranch = "",
    upstream = null,
    onpull,
  }: {
    open?: boolean;
    repoId: string | null;
    remotes?: { name: string }[];
    branches?: BranchInfo[];
    currentBranch?: string;
    upstream?: string | null;
    /** 任务中心（P12）：宿主立即关窗后台执行，回调同步返回即可。 */
    onpull: (
      remote: string | null,
      branch: string | null,
      mode: string,
      allowUnrelatedHistories: boolean
    ) => void | Promise<void>;
  } = $props();

  let remote = $state("");
  let branch = $state("");
  let mode = $state<"merge" | "rebase" | "ff_only">("merge");
  /** 当前分支与远端分支 merge_base 为 null（远端由网页端初始化等）时，
   *  merge 模式的 pull 必须带 --allow-unrelated-histories。 */
  let unrelated = $state(false);

  $effect(() => {
    if (open) {
      remote = remotes[0]?.name ?? "origin";
      // Default to the tracked upstream split into remote/branch.
      const up = upstream ?? "";
      const slash = up.indexOf("/");
      branch = up
        ? slash > 0
          ? up.slice(slash + 1)
          : up
        : currentBranch;
      // P10: remember the user's default pull strategy.
      mode = settings.pullStrategy;
    }
  });

  const MODES = [
    { id: "merge", label: "refs.pull.modeMerge", desc: "refs.pull.modeMergeDesc" },
    { id: "rebase", label: "refs.pull.modeRebase", desc: "refs.pull.modeRebaseDesc" },
    { id: "ff_only", label: "refs.pull.modeFfOnly", desc: "refs.pull.modeFfOnlyDesc" },
  ] as const;

  // 预检不相关历史（merge 模式才可能）：branch 是文本输入，防抖后
  // compare 当前分支与远端分支，merge_base 为 null → 横幅提示 + 带 flag。
  $effect(() => {
    const r = remote.trim();
    const b = branch.trim();
    if (!open || mode !== "merge" || !r || !b || !currentBranch || repoId === null) {
      unrelated = false;
      return;
    }
    const timer = setTimeout(() => {
      git
        .branchCompare(repoId, currentBranch, `${r}/${b}`)
        .then((c) => {
          unrelated = c.merge_base === null;
        })
        .catch(() => {
          unrelated = false;
        });
    }, 300);
    return () => clearTimeout(timer);
  });

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    try {
      await onpull(remote || null, branch.trim() || null, mode, mode === "merge" && unrelated);
      open = false;
    } catch {
      // toast surfaced by host
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>{t("refs.pull.title")}</Dialog.Title>
      <Dialog.Description>{t("refs.pull.desc", { branch: currentBranch })}</Dialog.Description>
    </Dialog.Header>
    <form class="space-y-3" onsubmit={submit}>
      <div class="grid grid-cols-2 gap-2">
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("refs.push.remote")}</span>
          <select bind:value={remote} class="h-9 w-full rounded-md border bg-background px-2 text-[13px]">
            {#each remotes as r (r.name)}
              <option value={r.name}>{r.name}</option>
            {:else}
              <option value="origin">origin</option>
            {/each}
          </select>
        </label>
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("refs.pull.branch")}</span>
          <input
            bind:value={branch}
            list="pull-branches"
            class="h-9 w-full rounded-md border bg-background px-2 font-mono text-[13px]"
          />
          <datalist id="pull-branches">
            {#each branches as b (b.name)}<option value={b.name}></option>{/each}
          </datalist>
        </label>
      </div>

      <div class="space-y-1.5">
        {#if mode === "merge" && unrelated}
          <p class="rounded-md bg-warning-surface px-2.5 py-2 text-xs text-warning dark:text-warning">
            {t("refs.pull.unrelated")}
          </p>
        {/if}
        {#each MODES as m (m.id)}
          <label class="flex cursor-pointer items-start gap-2 rounded-md border p-2.5 text-sm hover:bg-accent/50 {mode === m.id ? 'border-ring' : ''}">
            <input type="radio" class="mt-0.5" bind:group={mode} value={m.id} />
            <span>
              <span class="font-medium">{t(m.label)}</span>
              <span class="block text-xs text-muted-foreground">{t(m.desc)}</span>
            </span>
          </label>
        {/each}
      </div>

      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm">{t("refs.pull.confirm")}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
