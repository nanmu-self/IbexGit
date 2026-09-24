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
    /** Enable horizontal scrolling (P9 blame): the inner canvas grows to
     *  the widest row instead of clipping long lines. */
    scrollX = false,
    /** How the root gets its height in a flex-column parent:
     *  `fill` (default) — flex-1, takes a fixed share (dialog bodies, single
     *  list panels: HistoryView, FileHistoryList, BlameView);
     *  `content` — natural height, shrinkable when the shared viewport
     *  overflows, so each section scrolls internally (StatusSections:
     *  several lists + headers stacked in one flex column). Flex-shrink is
     *  set to the item count so the negative space distributes ∝ n²: big
     *  sections absorb the shrink, small ones keep their height (uniform
     *  shrink would squash a 5-file staged list into a sliver next to a
     *  500-file unstaged one). */
    sizing = "fill",
  }: {
    items: T[];
    itemHeight: number;
    overscan?: number;
    row: Snippet<[T, number]>;
    getKey?: (item: T, index: number) => string | number;
    onNearBottom?: () => void;
    nearBottomThreshold?: number;
    scrollX?: boolean;
    sizing?: "fill" | "content";
  } = $props();

  let scrollTop = $state(0);
  let viewport = $state(0);
  /** Distance from the bottom at the last near-bottom fire (debounce). */
  let lastFireAt = -1;
  let scroller: HTMLDivElement | null = null;

  /** Content-mode shrink weight (∝ n² with the auto basis). Static class
   *  variants can't express a dynamic number, hence the inline style. */
  const shrinkWeight = $derived(
    sizing === "content" ? Math.max(1, items.length) : null,
  );

  /** Center the given row in the viewport (jump-to-line support). */
  export function scrollToIndex(index: number): void {
    if (!scroller) return;
    scroller.scrollTop = Math.max(0, index * itemHeight - viewport / 2 + itemHeight / 2);
    // Sync internal state immediately: the scroll event only fires async, and
    // callers must be able to `await tick()` and see the new window rendered.
    scrollTop = scroller.scrollTop;
  }

  /** Minimal-scroll visibility for keyboard navigation: only scroll when the
   *  row sits outside the viewport, instead of re-centering every step. */
  export function ensureVisible(index: number): void {
    if (!scroller) return;
    const top = index * itemHeight;
    const bottom = top + itemHeight;
    const viewTop = scroller.scrollTop;
    const viewBottom = viewTop + viewport;
    if (top < viewTop) scroller.scrollTop = top;
    else if (bottom > viewBottom) scroller.scrollTop = bottom - viewport;
    else return;
    scrollTop = scroller.scrollTop;
  }

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
  bind:this={scroller}
  class="min-h-0 {sizing === 'fill' ? 'flex-1' : 'grow-0'} {scrollX
    ? 'overflow-auto'
    : 'overflow-y-auto'}"
  style={shrinkWeight === null ? undefined : `flex-shrink: ${shrinkWeight}`}
  bind:clientHeight={viewport}
  onscroll={handleScroll}
>
  <div class="relative w-full {scrollX ? 'min-w-max' : ''}" style="height: {items.length * itemHeight}px">
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
