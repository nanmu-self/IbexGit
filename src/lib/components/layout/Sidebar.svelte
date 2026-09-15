<script lang="ts">
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import RefPanel from "$lib/components/refs/RefPanel.svelte";
  import FileText from "@lucide/svelte/icons/file-text";
  import History from "@lucide/svelte/icons/history";
  import Tags from "@lucide/svelte/icons/tags";

  let { resizing = false }: { resizing?: boolean } = $props();

  const changed = $derived(repos.activeChanged);

  const views = [
    { id: "changes" as const, icon: FileText, label: t("sidebar.changes") },
    { id: "history" as const, icon: History, label: t("sidebar.history") },
    { id: "tags" as const, icon: Tags, label: t("sidebar.tagsView") },
  ];
</script>

<aside
  class="flex min-h-0 shrink-0 flex-col overflow-hidden border-r bg-muted/30 {!resizing
    ? 'transition-[width] duration-[120ms] ease-out'
    : ''}"
  style="width: {settings.sidebarWidth}px"
>
  <div class="flex flex-col gap-0.5 px-1.5 pt-1.5">
    {#each views as v (v.id)}
      <div
        class="flex items-center gap-2 rounded px-2 py-1 text-[13px] {repos.ui.view === v.id
          ? 'bg-accent font-medium text-foreground'
          : 'hover:bg-accent/60'}"
        role="button"
        tabindex="0"
        onclick={() => repos.updateUi({ view: v.id })}
        onkeydown={(e) => e.key === "Enter" && repos.updateUi({ view: v.id })}
      >
        <v.icon class="size-3.5" />
        <span class="flex-1">{v.label}</span>
        {#if v.id === "changes" && changed > 0}
          <span class="rounded-full bg-muted px-1.5 text-[10px] tabular-nums">{changed}</span>
        {/if}
      </div>
    {/each}
    <div class="mx-2 my-1 h-px bg-border"></div>
  </div>

  <div class="min-h-0 flex-1 overflow-y-auto px-1.5 pb-2">
    <RefPanel />
  </div>
</aside>
