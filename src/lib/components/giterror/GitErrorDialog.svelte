<script lang="ts">
  // Friendly git-failure dialog (P12): structured refusals (dirty worktree
  // family) surface here instead of a raw-stderr toast — VS Code-style
  // title + conflict file list + one-click stash (+ optional retry wired
  // by the op runner). Raw stderr stays one toggle away.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { t } from "$lib/i18n";
  import { giterr } from "$lib/stores/giterr.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import { git, normalizeError } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";

  const KNOWN_OPS = ["merge", "checkout", "switch", "pull", "rebase"];

  function opLabel(op: string | null): string {
    return op && KNOWN_OPS.includes(op) ? t(`giterr.op.${op}`) : t("giterr.op.fallback");
  }

  /** Stash the dirty changes, then re-run the refused op when wired. */
  async function stash(): Promise<void> {
    const repoId = repos.active?.id;
    if (!repoId || giterr.busy) return;
    giterr.busy = true;
    try {
      await git.stashPush(repoId, null);
      showToast("success", t("giterr.stashed"));
      const retry = giterr.retry;
      giterr.close();
      await retry?.();
    } catch (raw) {
      normalizeError(raw);
    } finally {
      giterr.busy = false;
    }
  }
</script>

<Dialog.Root bind:open={giterr.open}>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title class="flex items-center gap-2">
        <CircleAlert class="h-5 w-5 shrink-0 text-destructive" />
        <span>{giterr.info?.message ?? t("giterr.title")}</span>
      </Dialog.Title>
      <Dialog.Description>
        {#if giterr.info?.untracked}
          {t("giterr.untrackedDesc", { operation: opLabel(giterr.info?.operation ?? null) })}
        {:else}
          {t("giterr.desc", { operation: opLabel(giterr.info?.operation ?? null) })}
        {/if}
      </Dialog.Description>
    </Dialog.Header>

    {#if giterr.info && giterr.info.files.length > 0}
      <div
        class="max-h-40 overflow-auto rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs leading-5"
      >
        {#each giterr.info.files as f (f)}
          <div class="break-all">{f}</div>
        {/each}
      </div>
    {:else}
      <p class="text-sm text-muted-foreground">{t("giterr.noFiles")}</p>
    {/if}

    <p class="text-sm text-muted-foreground">
      {#if giterr.info?.untracked}
        {t("giterr.untrackedHint")}
      {:else}
        {t("giterr.hint")}
      {/if}
    </p>

    {#if giterr.showRaw && giterr.info}
      <pre
        class="max-h-48 overflow-auto rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs whitespace-pre-wrap"
        >{giterr.info.stderr}</pre
      >
    {/if}

    <Dialog.Footer>
      <Button variant="ghost" size="sm" onclick={() => (giterr.showRaw = !giterr.showRaw)}>
        {giterr.showRaw ? t("giterr.hideOutput") : t("giterr.showOutput")}
      </Button>
      <Button variant="outline" size="sm" onclick={() => giterr.close()}>
        {t("common.close")}
      </Button>
      {#if !giterr.info?.untracked}
        <Button size="sm" disabled={giterr.busy || !repos.active} onclick={stash}>
          {giterr.retry ? t("giterr.stashRetry") : t("giterr.stash")}
        </Button>
      {/if}
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
