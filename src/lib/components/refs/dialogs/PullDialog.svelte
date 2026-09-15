<script lang="ts">
  // Pull dialog (P6): remote / branch (default = upstream) + strategy
  // selection (merge / rebase / ff-only), per PLAN 合并/变基入口.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { t } from "$lib/i18n";
  import type { BranchInfo } from "$lib/git/bindings";

  let {
    open = $bindable(false),
    remotes = [],
    branches = [],
    currentBranch = "",
    upstream = null,
    busy = false,
    onpull,
  }: {
    open?: boolean;
    remotes?: { name: string }[];
    branches?: BranchInfo[];
    currentBranch?: string;
    upstream?: string | null;
    busy?: boolean;
    onpull: (remote: string | null, branch: string | null, mode: string) => Promise<void>;
  } = $props();

  let remote = $state("");
  let branch = $state("");
  let mode = $state<"merge" | "rebase" | "ff_only">("merge");

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
      mode = "merge";
    }
  });

  const MODES = [
    { id: "merge", label: "refs.pull.modeMerge", desc: "refs.pull.modeMergeDesc" },
    { id: "rebase", label: "refs.pull.modeRebase", desc: "refs.pull.modeRebaseDesc" },
    { id: "ff_only", label: "refs.pull.modeFfOnly", desc: "refs.pull.modeFfOnlyDesc" },
  ] as const;

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    try {
      await onpull(remote || null, branch.trim() || null, mode);
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
        <Button type="submit" size="sm" disabled={busy}>{t("refs.pull.confirm")}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
