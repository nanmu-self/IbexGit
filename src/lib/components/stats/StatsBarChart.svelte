<script lang="ts">
  // 轻量 SVG 柱状图（提交统计专用，零依赖）：
  // 桶少时撑满容器，桶多（总览全历史）时横向滚动，最小列宽保证可读。
  // 颜色全部走主题 CSS 变量，亮/暗主题自动跟随。
  let {
    buckets,
    formatLabel,
    height = 280,
  }: {
    buckets: { key: string; count: number }[];
    /** 规范键 → 展示标签（月 `2026-08`、日 `08-05`、小时 `14`）。 */
    formatLabel: (key: string) => string;
    height?: number;
  } = $props();

  const GUTTER = 44; // 左侧 y 轴标签
  const SLOT_MIN = 28; // 总览滚动时的最小列宽
  const PAD_TOP = 12;
  const PAD_BOTTOM = 22; // x 轴标签

  let cw = $state(0);

  const n = $derived(buckets.length);
  const svgW = $derived(Math.max(cw, GUTTER + n * SLOT_MIN));
  const slot = $derived(n === 0 ? SLOT_MIN : (svgW - GUTTER) / n);
  const plotH = $derived(height - PAD_TOP - PAD_BOTTOM);

  const maxCount = $derived(Math.max(1, ...buckets.map((b) => b.count)));

  /** 1/2/5×10^k 的"好看"刻度步长，全图约 4 条网格线。 */
  const step = $derived.by(() => {
    const raw = maxCount / 4;
    const pow = 10 ** Math.floor(Math.log10(Math.max(raw, 1e-9)));
    for (const m of [1, 2, 5, 10]) {
      if (m * pow >= raw) return m * pow;
    }
    return 10 * pow;
  });
  const top = $derived(Math.ceil(maxCount / step) * step);

  const ticks = $derived.by(() => {
    const out: number[] = [];
    for (let v = 0; v <= top; v += step) out.push(v);
    return out;
  });

  /** x 轴标签抽样：列窄时隔几个标一个。 */
  const labelEvery = $derived(Math.max(1, Math.ceil(52 / slot)));

  let hovered = $state<number | null>(null);

  const barW = $derived(Math.min(56, Math.max(6, slot * 0.62)));

  function x(i: number): number {
    return GUTTER + i * slot + (slot - barW) / 2;
  }

  /** 顶部圆角的柱体路径；零计数的桶不画。 */
  function barPath(i: number, count: number): string {
    const h = (count / top) * plotH;
    if (h < 0.5) return "";
    const bx = x(i);
    const by = PAD_TOP + plotH - h;
    const r = Math.min(3, h, barW / 2);
    return (
      `M${bx},${by + h}` +
      `L${bx},${by + r}Q${bx},${by} ${bx + r},${by}` +
      `L${bx + barW - r},${by}Q${bx + barW},${by} ${bx + barW},${by + r}` +
      `L${bx + barW},${by + h}Z`
    );
  }

  const summary = $derived(
    `${n} buckets, max ${maxCount}`,
  );
</script>

<div class="relative min-w-0 flex-1 overflow-hidden" bind:clientWidth={cw}>
  <div class="overflow-x-auto pb-1">
    <svg
      width={svgW}
      {height}
      role="img"
      aria-label={summary}
      class="block select-none"
    >
      <!-- 网格线 + y 轴标签 -->
      {#each ticks as v (v)}
        <line
          x1={GUTTER}
          x2={svgW}
          y1={PAD_TOP + (1 - v / top) * plotH}
          y2={PAD_TOP + (1 - v / top) * plotH}
          stroke="var(--border)"
          stroke-width="1"
          shape-rendering="crispEdges"
        />
        <text
          x={GUTTER - 8}
          y={PAD_TOP + (1 - v / top) * plotH + 3}
          text-anchor="end"
          font-size="10"
          fill="var(--muted-foreground)"
        >
          {v}
        </text>
      {/each}

      <!-- 柱体 + 整列悬停热区 -->
      {#each buckets as b, i (b.key)}
        <path d={barPath(i, b.count)} fill="var(--primary)" opacity={hovered === i ? 1 : 0.72}
          class="transition-opacity" />
        <rect
          x={GUTTER + i * slot}
          y={PAD_TOP}
          width={slot}
          height={plotH}
          fill="transparent"
          role="presentation"
          onmouseenter={() => (hovered = i)}
          onmouseleave={() => (hovered = null)}
        />
      {/each}

      <!-- x 轴标签 -->
      {#each buckets as b, i (b.key)}
        {#if i % labelEvery === 0}
          <text
            x={GUTTER + i * slot + slot / 2}
            y={height - 6}
            text-anchor="middle"
            font-size="10"
            fill="var(--muted-foreground)"
          >
            {formatLabel(b.key)}
          </text>
        {/if}
      {/each}
    </svg>
  </div>

  <!-- 悬停浮层：定位在柱顶上方；钳制在容器内避免被 overflow-hidden
       裁掉（最高柱/首尾列时恰好贴边） -->
  {#if hovered !== null && buckets[hovered]}
    {@const b = buckets[hovered]}
    {@const h = (b.count / top) * plotH}
    {@const cx = Math.max(64, Math.min(svgW - 64, GUTTER + hovered * slot + slot / 2))}
    {@const cy = Math.max(34, PAD_TOP + plotH - h - 6)}
    <div
      class="pointer-events-none absolute z-10 -translate-x-1/2 -translate-y-full rounded-md border bg-popover px-2 py-1 text-xs shadow-md"
      style:left={`${cx}px`}
      style:top={`${cy}px`}
    >
      <span class="text-muted-foreground">{formatLabel(b.key)}</span>
      <span class="ml-1.5 font-medium tabular-nums">{b.count}</span>
    </div>
  {/if}
</div>
