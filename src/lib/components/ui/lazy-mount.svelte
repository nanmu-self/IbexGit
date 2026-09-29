<script lang="ts">
  /**
   * LazyMount — trigger-loaded wrapper for globally mounted dialogs whose
   * open state lives in a store (FileInspectDialog / GitErrorDialog /
   * ReportDialog). Renders nothing until `active` first becomes true, then
   * dynamic-imports the real component (shared cache in $lib/lazy-views.ts)
   * and keeps it mounted permanently — the inner component owns its
   * open/close via the same store flag (sticky: no unmount on close, so
   * internal state survives). These dialogs are safe to defer because they
   * are purely store-driven; hosts with mount-time event wiring
   * (RefsDialogsHost, NetDialogsHost) must stay eagerly imported.
   */
  import { loadView } from "$lib/lazy-views";
  import type { Component } from "svelte";

  type Props = {
    active: boolean;
    loader: () => Promise<{ default: Component }>;
  };
  let { active, loader }: Props = $props();

  let Comp = $state<Component | null>(null);

  $effect(() => {
    if (!active || Comp) return;
    let alive = true;
    loadView(loader)
      .then((c) => {
        if (alive) Comp = c;
      })
      .catch(() => {
        // Leave unloaded; the next activation of `active` retries.
      });
    return () => {
      alive = false;
    };
  });
</script>

{#if Comp}
  <Comp />
{/if}
