<script lang="ts">
  // Backup refs list (P6): track-B recovery points under
  // refs/ibexgit/backups/ + the orphan-cleanup entry (delete = the kept
  // commits become garbage-collectable again).
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { t } from "$lib/i18n";
  import type { BackupRef } from "$lib/git/bindings";
  import Forward from "@lucide/svelte/icons/rotate-ccw";

  let {
    open = $bindable(false),
    refs = [],
    loading = false,
    busy = false,
    ondelete,
    onrestore,
  }: {
    open?: boolean;
    refs?: BackupRef[];
    loading?: boolean;
    busy?: boolean;
    ondelete: (names: string[]) => Promise<void>;
    onrestore: (name: string) => void;
  } = $props();

  let selected = $state(new Set<string>());

  $effect(() => {
    if (open) selected = new Set();
  });

  function toggle(name: string, on: boolean): void {
    const next = new Set(selected);
    if (on) next.add(name);
    else next.delete(name);
    selected = next;
  }

  async function deleteSelected(): Promise<void> {
    try {
      await ondelete([...selected]);
      open = false;
    } catch {
      /* toast surfaced */
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{t("refs.backups.title")}</Dialog.Title>
      <Dialog.Description>{t("refs.backups.desc")}</Dialog.Description>
    </Dialog.Header>

    <div class="max-h-72 min-h-24 overflow-y-auto rounded-md border">
      {#if loading}
        <div class="p-4 text-center text-xs text-muted-foreground">{t("common.loading")}</div>
      {:else if refs.length === 0}
        <div class="p-4 text-center text-xs text-muted-foreground">{t("refs.backups.empty")}</div>
      {:else}
        {#each refs as r (r.full_name)}
          <div class="flex items-center gap-2 border-b px-3 py-1.5 text-xs last:border-b-0">
            <Checkbox checked={selected.has(r.full_name)} onCheckedChange={(v) => toggle(r.full_name, v === true)} />
            <Forward class="size-3.5 shrink-0 text-muted-foreground" />
            <div class="min-w-0 flex-1">
              <div class="truncate font-medium">{r.name}</div>
              <div class="truncate text-muted-foreground">
                {r.short_hash} · {r.subject || "—"} · {r.date}
              </div>
            </div>
            <Button variant="ghost" size="xs" onclick={() => onrestore(r.full_name)}>
              {t("refs.backups.restore")}
            </Button>
          </div>
        {/each}
      {/if}
    </div>

    <div class="flex items-center justify-between">
      <span class="text-xs text-muted-foreground">{t("refs.backups.gcHint")}</span>
      <Dialog.Footer class="p-0">
        <Button variant="ghost" size="sm" onclick={() => (open = false)}>{t("common.close")}</Button>
        <Button
          variant="destructive"
          size="sm"
          disabled={busy || selected.size === 0}
          onclick={deleteSelected}
        >
          {t("refs.backups.delete", { n: selected.size })}
        </Button>
      </Dialog.Footer>
    </div>
  </Dialog.Content>
</Dialog.Root>
