<script lang="ts">
  // Clean preview (P6): list removable untracked files/dirs, per-item
  // selection, confirm → delete with a recovery snapshot (undoable).
  // Nothing is ever removed without explicit confirmation.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { t } from "$lib/i18n";

  let {
    open = $bindable(false),
    paths = [],
    loading = false,
    busy = false,
    onconfirm,
  }: {
    open?: boolean;
    paths?: string[];
    loading?: boolean;
    busy?: boolean;
    /** Returns a promise; the dialog closes only on success. */
    onconfirm: (selected: string[]) => Promise<void>;
  } = $props();

  let selected = $state(new Set<string>());

  $effect(() => {
    if (open) selected = new Set();
  });

  const allSelected = $derived(paths.length > 0 && selected.size === paths.length);
  const selectionCount = $derived(selected.size);

  function toggle(path: string, on: boolean): void {
    const next = new Set(selected);
    if (on) next.add(path);
    else next.delete(path);
    selected = next;
  }

  async function confirm(): Promise<void> {
    try {
      await onconfirm([...selected]);
      open = false;
    } catch {
      // toast already surfaced
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>{t("refs.clean.title")}</Dialog.Title>
      <Dialog.Description>{t("refs.clean.desc")}</Dialog.Description>
    </Dialog.Header>

    <div class="max-h-64 min-h-24 overflow-y-auto rounded-md border">
      {#if loading}
        <div class="p-4 text-center text-xs text-muted-foreground">{t("common.loading")}</div>
      {:else if paths.length === 0}
        <div class="p-4 text-center text-xs text-muted-foreground">{t("refs.clean.empty")}</div>
      {:else}
        {#each paths as p (p)}
          <label class="flex items-center gap-2 border-b px-3 py-1.5 text-[13px] last:border-b-0 hover:bg-accent/40">
            <Checkbox
              checked={selected.has(p)}
              onCheckedChange={(v) => toggle(p, v === true)}
            />
            <span class="truncate font-mono text-xs" title={p}>{p}</span>
          </label>
        {/each}
      {/if}
    </div>

    <div class="flex items-center justify-between text-xs text-muted-foreground">
      <span>{t("refs.clean.warn")}</span>
      {#if paths.length > 0}
        <button
          type="button"
          class="text-xs text-foreground underline-offset-2 hover:underline"
          onclick={() => (selected = allSelected ? new Set() : new Set(paths))}
        >
          {allSelected ? t("refs.clean.selectNone") : t("refs.clean.selectAll")}
        </button>
      {/if}
    </div>

    <Dialog.Footer>
      <Button variant="ghost" size="sm" onclick={() => (open = false)}>
        {t("common.cancel")}
      </Button>
      <Button
        variant="destructive"
        size="sm"
        disabled={busy || selectionCount === 0}
        onclick={confirm}
      >
        {t("refs.clean.confirm", { n: selectionCount })}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
