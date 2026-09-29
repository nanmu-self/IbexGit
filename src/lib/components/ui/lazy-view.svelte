<script lang="ts">
  /**
   * LazyView — dynamic-import wrapper for the mutually exclusive main-area
   * view branches in +page.svelte (workspace / history / tags / repos tab).
   * Rendering state comes from the shared module cache in $lib/lazy-views.ts:
   * an already-loaded (or warmed-up) view renders synchronously with no
   * pending flash, even after this component instance is remounted. Failed
   * loads are not cached — the next branch activation retries. Pairs with
   * `lazy-mount.svelte` which handles trigger-loaded dialogs.
   */
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import { loadView, peekView } from "$lib/lazy-views";
  import { t } from "$lib/i18n";
  import type { Component } from "svelte";

  type Props = { loader: () => Promise<{ default: Component }> };
  let { loader }: Props = $props();

  let Comp = $state<Component | null>(null);
  let failed = $state(false);

  $effect(() => {
    const l = loader;
    let alive = true;
    failed = false;
    const hit = peekView(l);
    if (hit) {
      Comp = hit;
      return;
    }
    Comp = null;
    loadView(l)
      .then((c) => {
        if (alive) Comp = c;
      })
      .catch(() => {
        if (alive) failed = true;
      });
    return () => {
      alive = false;
    };
  });
</script>

{#if Comp}
  <Comp />
{:else if failed}
  <div class="flex flex-1 items-center justify-center text-sm text-muted-foreground">
    {t("common.viewLoadFailed")}
  </div>
{:else}
  <div class="flex flex-1 items-center justify-center">
    <LoaderCircle class="size-4 animate-spin text-muted-foreground" />
  </div>
{/if}
