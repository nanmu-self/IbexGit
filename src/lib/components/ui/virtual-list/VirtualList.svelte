<script lang="ts" generics="T">
  import type { Snippet } from "svelte";

  let {
    items,
    itemHeight,
    overscan = 6,
    row,
    getKey,
    /** Fired when the viewport approaches the bottom (infinite scroll). */
    onNearBottom = undefined,
    nearBottomThreshold = 600,
  }: {
    items: T[];
    itemHeight: number;
    overscan?: number;
    row: Snippet<[T, number]>;
    getKey?: (item: T, index: number) => string | number;
    onNearBottom?: () => void;
    nearBottomThreshold?: number;
  } = $props();

  let scrollTop = $state(0);
  let viewport = $state(0);
  /** Distance from the bottom at the last near-bottom fire (debounce). */
  let lastFireAt = -1;

  const start = $derived(
    Math.max(0, Math.floor(scrollTop / itemHeight) - overscan)
  );
  const end = $derived(
    Math.min(items.length, Math.ceil((scrollTop + viewport) / itemHeight) + overscan)
  );
  const visible = $derived(
    items.slice(start, end).map((item, i) => ({ item, index: start + i }))
  );

  function handleScroll(e: Event): void {
    const el = e.currentTarget as HTMLElement;
    scrollTop = el.scrollTop;
    if (!onNearBottom) return;
    const fromBottom = items.length * itemHeight - (scrollTop + viewport);
    if (fromBottom > nearBottomThreshold) {
      lastFireAt = -1; // scrolled away again → re-arm
      return;
    }
    if (lastFireAt === -1) {
      lastFireAt = scrollTop;
      onNearBottom();
    }
  }
</script>

<div
  class="min-h-0 flex-1 overflow-y-auto"
  bind:clientHeight={viewport}
  onscroll={handleScroll}
>
  <div class="relative w-full" style="height: {items.length * itemHeight}px">
    {#each visible as v (getKey ? getKey(v.item, v.index) : v.index)}
      <div
        class="absolute left-0 right-0"
        style="top: {v.index * itemHeight}px; height: {itemHeight}px"
      >
        {@render row(v.item, v.index)}
      </div>
    {/each}
  </div>
</div>
