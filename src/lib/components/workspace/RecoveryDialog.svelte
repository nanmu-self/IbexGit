<script lang="ts">
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { EmptyState } from "$lib/components/ui/empty-state";
  import { t } from "$lib/i18n";
  import type { RecoveryEntry } from "$lib/git";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
  import Undo2 from "@lucide/svelte/icons/undo-2";
  import Trash2 from "@lucide/svelte/icons/trash-2";

  let {
    open = $bindable(false),
    entries,
    busyId = null,
    onrestore,
    ondelete,
  }: {
    open?: boolean;
    entries: RecoveryEntry[];
    busyId?: string | null;
    onrestore: (id: string) => void;
    ondelete: (id: string) => void;
  } = $props();

  function formatTime(ms: number | null): string {
    return new Date(ms ?? 0).toLocaleString();
  }

  function formatSize(bytes: number | null): string {
    const b = bytes ?? 0;
    if (b < 1024) return `${b} B`;
    if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
    return `${(b / 1024 / 1024).toFixed(1)} MB`;
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{t("recovery.title")}</Dialog.Title>
      <Dialog.Description>{t("recovery.description")}</Dialog.Description>
    </Dialog.Header>

    {#if entries.length === 0}
      <EmptyState title={t("recovery.empty")} hint={t("recovery.emptyHint")} compact />
    {:else}
      <div class="max-h-80 space-y-1.5 overflow-y-auto pr-1">
        {#each entries as entry (entry.id)}
          <div class="rounded-md border p-2 text-[13px]">
            <div class="flex items-center gap-2">
              <Undo2 class="size-3.5 shrink-0 text-muted-foreground" />
              <span class="min-w-0 flex-1 truncate font-medium">{formatTime(entry.created_at_ms)}</span>
              {#if entry.warnings.length > 0}
                <TriangleAlert class="size-3.5 shrink-0 text-amber-500" title={entry.warnings.join("\n")} />
              {/if}
            </div>
            <div class="mt-0.5 flex items-center gap-2 pl-6 text-xs text-muted-foreground">
              <span>{t("recovery.scope." + entry.scope)}</span>
              <span>·</span>
              <span>{t("recovery.fileCount", { n: entry.file_count })}</span>
              <span>·</span>
              <span>{formatSize(entry.size_bytes)}</span>
              <div class="ml-auto flex items-center gap-1">
                <Button
                  variant="ghost"
                  size="xs"
                  disabled={busyId !== null}
                  onclick={() => onrestore(entry.id)}
                >
                  {busyId === entry.id ? t("common.loading") : t("recovery.restore")}
                </Button>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  disabled={busyId !== null}
                  title={t("recovery.delete")}
                  onclick={() => ondelete(entry.id)}
                >
                  <Trash2 class="size-3.5" />
                </Button>
              </div>
            </div>
            {#if entry.warnings.length > 0}
              <div class="mt-1 space-y-0.5 pl-6 text-[11px] text-amber-600 dark:text-amber-500">
                {#each entry.warnings as warning}
                  <div class="break-all">{warning}</div>
                {/each}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </Dialog.Content>
</Dialog.Root>
