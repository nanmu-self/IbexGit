<script lang="ts">
  // GraphRenderer (P5): draws ONE commit row as self-contained SVG —
  // nodes, pass-through pipes, and the upper/lower halves of edges, per
  // the row model computed by the Rust GraphLayout (core/graph.rs).
  import type { GraphRow } from "$lib/git";

  let {
    row,
    lanes,
    rowHeight = 28,
    colWidth = 14,
    isHead = false,
    onGraphClick,
  }: {
    row: GraphRow;
    /** Total columns the graph currently spans (≥ row.lane + 1). */
    lanes: number;
    rowHeight?: number;
    colWidth?: number;
    /** HEAD pointer sits on this row. */
    isHead?: boolean;
    onGraphClick?: (e: MouseEvent) => void;
  } = $props();

  const PALETTE = [
    "#0ea5e9", // sky
    "#f59e0b", // amber
    "#8b5cf6", // violet
    "#10b981", // emerald
    "#ef4444", // red
    "#06b6d4", // cyan
    "#f97316", // orange
    "#14b8a6", // teal
    "#d946ef", // fuchsia
    "#84cc16", // lime
  ];

  const width = $derived(Math.max(1, lanes) * colWidth);
  const mid = $derived(rowHeight / 2);

  const x = $derived((lane: number): number => lane * colWidth + colWidth / 2);

  function color(lane: number): string {
    return PALETTE[lane % PALETTE.length];
  }

  const pipes = $derived(row.edges.filter((e) => !e.from_node && !e.to_node));
  const upper = $derived(row.edges.filter((e) => e.to_node));
  const lower = $derived(row.edges.filter((e) => e.from_node));
</script>

<svg
  {width}
  height={rowHeight}
  viewBox="0 0 {width} {rowHeight}"
  class="shrink-0 select-none"
  onclick={onGraphClick}
  role="presentation"
>
  {#each pipes as e (e.to)}
    <line
      x1={x(e.from)}
      y1="0"
      x2={x(e.to)}
      y2={rowHeight}
      stroke={color(e.to)}
      stroke-width="1.5"
    />
  {/each}
  {#each upper as e (e.to)}
    {#if e.from === row.lane}
      <line
        x1={x(e.from)}
        y1="0"
        x2={x(row.lane)}
        y2={mid}
        stroke={color(row.lane)}
        stroke-width="1.5"
      />
    {:else}
      <path
        d="M {x(e.from)} 0 Q {x(e.from)} {mid} {x(row.lane)} {mid}"
        fill="none"
        stroke={color(row.lane)}
        stroke-width="1.5"
      />
    {/if}
  {/each}
  {#each lower as e (e.to)}
    {#if e.from === row.lane && e.to === row.lane}
      <line
        x1={x(row.lane)}
        y1={mid}
        x2={x(e.to)}
        y2={rowHeight}
        stroke={color(e.to)}
        stroke-width="1.5"
      />
    {:else if e.from === row.lane}
      <path
        d="M {x(row.lane)} {mid} Q {x(row.lane)} {rowHeight} {x(e.to)} {rowHeight}"
        fill="none"
        stroke={color(e.to)}
        stroke-width="1.5"
      />
    {/if}
  {/each}
  <circle
    cx={x(row.lane)}
    cy={mid}
    r={isHead ? 4.5 : 3.5}
    fill={color(row.lane)}
    stroke={isHead ? "var(--color-foreground)" : "var(--color-background)"}
    stroke-width={isHead ? 1.5 : 1}
  />
</svg>
