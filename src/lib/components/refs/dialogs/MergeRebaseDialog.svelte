<script lang="ts">
  // Merge / Rebase dialog (P6): pick a target ref, preview the commits
  // involved (dry-run via rev-list), then execute. Conflicts surface as
  // git errors until the P8 visual flow lands.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { t } from "$lib/i18n";
  import { git, normalizeError, type CommitInfo } from "$lib/git";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

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
    onexecute: (mode: "merge" | "rebase", target: string, ffOnly: boolean) => Promise<void>;
  } = $props();

  let target = $state("");
  let ffOnly = $state(false);
  let preview = $state<CommitInfo[] | null>(null);
  let previewLoading = $state(false);

  $effect(() => {
    if (open) {
      target = initialTarget;
      ffOnly = false;
      preview = null;
    }
  });

  // Dry-run preview: the commits the operation will bring in / replay.
  $effect(() => {
    const tgt = target.trim();
    if (!open || !tgt || tgt === currentBranch) {
      preview = null;
      return;
    }
    previewLoading = true;
    const range = mode === "merge" ? `HEAD..${tgt}` : `${tgt}..HEAD`;
    git
      .revList(repoId ?? "", range, 50, 0)
      .then((c) => (preview = c))
      .catch(() => (preview = null))
      .finally(() => (previewLoading = false));
  });

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    try {
      await onexecute(mode, target.trim(), ffOnly);
      open = false;
    } catch (e) {
      normalizeError(e);
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>
        {mode === "merge" ? t("refs.merge.title") : t("refs.rebase.title")}
      </Dialog.Title>
      <Dialog.Description>
        {mode === "merge"
          ? t("refs.merge.desc", { branch: currentBranch })
          : t("refs.rebase.desc", { branch: currentBranch })}
      </Dialog.Description>
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
          <LoaderCircle class="size-3.5 animate-spin" /> {t("common.loading")}
        </div>
      {:else if preview && preview.length > 0}
        <div>
          <div class="mb-1 text-xs text-muted-foreground">
            {mode === "merge"
              ? t("refs.merge.preview", { n: preview.length })
              : t("refs.rebase.preview", { n: preview.length })}
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

      {#if mode === "merge"}
        <label class="flex items-center gap-2 text-[13px]">
          <input type="checkbox" bind:checked={ffOnly} class="size-4" />
          {t("refs.merge.ffOnly")}
        </label>
      {:else}
        <p class="rounded-md bg-amber-500/10 px-2.5 py-2 text-xs text-amber-600 dark:text-amber-500">
          {t("refs.rebase.warn")}
        </p>
      {/if}

      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={busy || !target} variant={mode === "rebase" ? "destructive" : "default"}>
          {mode === "merge" ? t("refs.merge.confirm") : t("refs.rebase.confirm")}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
