<script lang="ts">
  /**
   * DiffViewer (P4) — 只消费 DiffModel，不接触原始 diff 文本（ADR-003）。
   * - 统一 / 双栏视图，变高虚拟滚动
   * - 行级选择 + hunk 级操作按钮（stage/discard/unstage 经父级走 IPC）
   * - 未变更块折叠 / 展开上下文（--unified=N 重取由父级完成）
   * - Shiki 语法高亮（懒加载、按 hunk 缓存）、词级差异、空白开关
   */
  import type { DiffModel, DiffLine, LineSelection } from "$lib/git/bindings";
  import { t } from "$lib/i18n";
  import { settings } from "$lib/stores/settings.svelte";
  import {
    buildFileContexts,
    buildRows,
    computeOffsets,
    findIndexAt,
    type ExpandedRuns,
    type Row,
    type ViewMode,
    type FileCtx,
  } from "$lib/diff/rows";
  import {
    getHunkTokens,
    langIdFor,
    type ShikiTheme,
    type ThemedTokenLike,
  } from "$lib/diff/highlight";
  import EmptyState from "$lib/components/ui/empty-state/EmptyState.svelte";
  import ImageDiff from "$lib/components/diff/ImageDiff.svelte";
  import FileText from "@lucide/svelte/icons/file-text";
  import Columns2 from "@lucide/svelte/icons/columns-2";
  import Rows3 from "@lucide/svelte/icons/rows-3";
  import WholeWord from "@lucide/svelte/icons/whole-word";
  import Sparkles from "@lucide/svelte/icons/sparkles";

  export type LineOp = "stage" | "discard" | "unstage";

  interface Props {
    model: DiffModel | null;
    loading: boolean;
    repoId: string | null;
    ignoreWhitespace: boolean;
    onlineop: (op: LineOp, path: string, selections: LineSelection[]) => void;
    onexpand: (path: string, dir: "up" | "down" | "all") => void;
    onignorewschange: (value: boolean) => void;
  }

  let { model, loading, repoId, ignoreWhitespace, onlineop, onexpand, onignorewschange }: Props =
    $props();

  const OVERSCAN = 12;
  /** Approximate monospace char width for the horizontal scroll width. */
  const CHAR_W = 7.5;
  const GUTTER_W = 100;

  // ---------- view state ----------
  let scrollTop = $state(0);
  let viewportH = $state(0);
  let expandedRuns = $state<ExpandedRuns>(new Set());
  let exhausted = $state(new Map<string, { up: boolean; down: boolean }>());
  let selections = $state(new Map<string, Set<string>>());

  const viewMode = $derived<ViewMode>(settings.diffViewMode);
  const showWhitespace = $derived(settings.diffShowWhitespace);
  const syntax = $derived(settings.diffSyntax);
  const isDark = $derived(
    settings.theme === "dark" ||
      (settings.theme === "system" &&
        window.matchMedia("(prefers-color-scheme: dark)").matches),
  );
  const theme: ShikiTheme = $derived(isDark ? "github-dark" : "github-light");

  // ---------- ops gating (DiffModel source 驱动操作权限, PLAN P4) ----------
  const ops = $derived.by(() => {
    const src = model?.source;
    if (src === "worktree") return { primary: "stage" as LineOp, secondary: "discard" as LineOp };
    if (src === "staged") return { primary: "unstage" as LineOp, secondary: null };
    return { primary: null, secondary: null };
  });

  // ---------- rows (virtualization core) ----------
  const fileContexts = $derived(model ? buildFileContexts(model) : []);
  const rows = $derived.by<Row[]>(() => {
    if (!model) return [];
    return buildRows(model, { mode: viewMode, expandedRuns, exhausted });
  });
  const offsets = $derived(computeOffsets(rows));
  const totalH = $derived(offsets[offsets.length - 1] ?? 0);
  const maxLineLen = $derived.by(() => {
    let m = 40;
    for (const row of rows) {
      if (row.t === "line") m = Math.max(m, row.line.content.length);
      else if (row.t === "pair") {
        if (row.left) m = Math.max(m, row.left.content.length);
        if (row.right) m = Math.max(m, row.right.content.length);
      }
    }
    return m;
  });
  /** Inner width so horizontal scroll reaches the longest visible line. */
  const contentW = $derived(
    Math.max(600, (maxLineLen + 4) * CHAR_W * (viewMode === "split" ? 0.55 : 1) + GUTTER_W),
  );
  const startIdx = $derived(Math.max(0, findIndexAt(offsets, scrollTop) - OVERSCAN));
  const endIdx = $derived(
    Math.min(rows.length, findIndexAt(offsets, scrollTop + viewportH) + OVERSCAN),
  );
  const visible = $derived.by(() => {
    const out: { row: Row; i: number; top: number; h: number }[] = [];
    for (let i = startIdx; i < endIdx; i++) {
      out.push({ row: rows[i], i, top: offsets[i], h: offsets[i + 1] - offsets[i] });
    }
    return out;
  });

  function rowKey(row: Row, i: number): string {
    switch (row.t) {
      case "file-header":
        return `fh${row.file.index}`;
      case "binary":
        return `bin${row.file.index}`;
      case "image":
        return `img${row.file.index}`;
      case "hunk-header":
        return `hh${row.file.index}:${row.hunkIndex}`;
      case "expand":
        return `ex${row.file.index}:${row.hunkIndex}:${row.dir}`;
      case "line":
        return `l${row.file.index}:${row.hunkIndex}:${row.lineIndex}`;
      case "pair":
        return `p${row.file.index}:${row.hunkIndex}:${row.leftIndex ?? "e"}:${row.rightIndex ?? "e"}:${i}`;
      case "collapse":
        return `c${row.runKey}`;
    }
  }

  // Reset transient state when the SELECTED FILE changes (not on every
  // watcher refresh — in-progress line selections must survive those).
  // Also detect exhausted expansion: a refetch that produced an identical
  // hunk signature means there is no more context to fetch.
  let prevSigs = new Map<string, string>();
  let pendingExpand: { path: string; dir: "up" | "down" } | null = null;
  let lastSelKey: string | null = null;
  const selKey = $derived(
    model ? `${repoId}:${model.source}:${fileContexts[0]?.path ?? ""}` : "",
  );
  $effect(() => {
    void model?.id;
    if (selKey !== lastSelKey) {
      lastSelKey = selKey;
      expandedRuns = new Set();
      selections = new Map();
      exhausted = new Map();
      prevSigs.clear();
    }
    if (model) {
      for (const fc of fileContexts) {
        const sig = hunkSig(fc);
        if (pendingExpand && pendingExpand.path === fc.path && prevSigs.get(fc.path) === sig) {
          const ex = exhausted.get(fc.path) ?? { up: false, down: false };
          if (pendingExpand.dir === "up") ex.up = true;
          else ex.down = true;
          exhausted.set(fc.path, ex);
          exhausted = new Map(exhausted);
        }
        prevSigs.set(fc.path, sig);
      }
    }
    pendingExpand = null;
  });

  function hunkSig(fc: FileCtx): string {
    return fc.file.hunks
      .map((h) => `${h.old_start}:${h.old_count}:${h.new_start}:${h.new_count}`)
      .join(",");
  }

  // ---------- highlighting (visible hunks only) ----------
  let tokenVersion = $state(0);
  const tokenCache = new Map<string, { theme: ShikiTheme; tokens: ThemedTokenLike[][] | null }>();

  const visibleHunks = $derived.by(() => {
    const set = new Map<string, { fc: FileCtx; hunkIndex: number }>();
    for (const v of visible) {
      const r = v.row;
      if (r.t === "line" || r.t === "pair" || r.t === "hunk-header" || r.t === "collapse") {
        const key = `${r.file.index}:${r.hunkIndex}`;
        if (!set.has(key)) set.set(key, { fc: r.file, hunkIndex: r.hunkIndex });
      }
    }
    return set;
  });

  $effect(() => {
    if (!syntax || !model) return;
    void theme;
    void tokenVersion;
    for (const [key, { fc, hunkIndex }] of visibleHunks) {
      const cached = tokenCache.get(key);
      if (cached && cached.theme === theme) continue;
      const lang = langIdFor(fc.path);
      if (!lang) {
        tokenCache.set(key, { theme, tokens: null });
        continue;
      }
      const lines = fc.file.hunks[hunkIndex].lines.map((l) => l.content);
      getHunkTokens(lines, lang, theme).then((tokens) => {
        tokenCache.set(key, { theme, tokens });
        tokenVersion += 1;
      });
    }
  });

  function tokensFor(fileIndex: number, hunkIndex: number, themeNow: ShikiTheme) {
    void tokenVersion;
    const hit = tokenCache.get(`${fileIndex}:${hunkIndex}`);
    if (!hit || hit.theme !== themeNow) return null;
    return hit.tokens;
  }

  // ---------- selection ----------
  let anchor: { path: string; hunk: number; line: number } | null = null;

  function selSet(path: string): Set<string> {
    let s = selections.get(path);
    if (!s) {
      s = new Set();
      selections.set(path, s);
    }
    return s;
  }

  function onLineClick(
    fc: FileCtx,
    hunkIndex: number,
    lineIndex: number,
    line: DiffLine,
    e: MouseEvent,
  ): void {
    if (!ops.primary && !ops.secondary) return;
    if (!fc.lineOpsAllowed) return;
    if (line.kind !== "add" && line.kind !== "remove") return;
    const key = `${hunkIndex}:${lineIndex}`;
    const set = selSet(fc.path);
    if (e.shiftKey && anchor && anchor.path === fc.path && anchor.hunk === hunkIndex) {
      const [lo, hi] =
        anchor.line < lineIndex ? [anchor.line, lineIndex] : [lineIndex, anchor.line];
      for (let i = lo; i <= hi; i++) {
        const l = fc.file.hunks[hunkIndex].lines[i];
        if (l && (l.kind === "add" || l.kind === "remove")) set.add(`${hunkIndex}:${i}`);
      }
    } else if (set.has(key)) {
      set.delete(key);
    } else {
      set.add(key);
    }
    anchor = { path: fc.path, hunk: hunkIndex, line: lineIndex };
    selections = new Map(selections); // trigger reactivity
  }

  function selectionsOf(fc: FileCtx): LineSelection[] {
    const set = selections.get(fc.path);
    if (!set || set.size === 0) return [];
    return [...set].map((k) => {
      const [h, l] = k.split(":");
      return { hunk: Number(h), line: Number(l) };
    });
  }

  /** Whole-hunk op: apply to every changed line of the hunk one-shot. */
  function hunkOp(op: LineOp, fc: FileCtx, hunkIndex: number): void {
    const lines: LineSelection[] = [];
    fc.file.hunks[hunkIndex].lines.forEach((l, i) => {
      if (l.kind === "add" || l.kind === "remove") lines.push({ hunk: hunkIndex, line: i });
    });
    if (lines.length > 0) onlineop(op, fc.path, lines);
  }

  function selectedOp(op: LineOp, fc: FileCtx): void {
    const sels = selectionsOf(fc);
    if (sels.length === 0) return;
    onlineop(op, fc.path, sels);
    selections.get(fc.path)?.clear();
    selections = new Map(selections);
  }

  function toggleExpand(runKey: string): void {
    if (expandedRuns.has(runKey)) expandedRuns.delete(runKey);
    else expandedRuns.add(runKey);
    expandedRuns = new Set(expandedRuns);
  }

  function doExpand(fc: FileCtx, dir: "up" | "down"): void {
    pendingExpand = { path: fc.path, dir };
    onexpand(fc.path, dir);
  }

  // ---------- content rendering (escape-first HTML) ----------
  function esc(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  function wsMark(s: string): string {
    if (!showWhitespace) return s;
    return s.replace(/\t/g, '<span class="text-muted-foreground/40">→</span>')
      .replace(/ /g, '<span class="text-muted-foreground/40">·</span>');
  }

  function fontStyleCss(style: number | undefined): string {
    if (!style) return "";
    const bits: string[] = [];
    if (style & 1) bits.push("font-style:italic");
    if (style & 2) bits.push("font-weight:600");
    if (style & 4) bits.push("text-decoration:underline");
    return bits.join(";");
  }

  function renderContent(
    fc: FileCtx,
    hunkIndex: number,
    lineIndex: number | null,
    line: DiffLine | null,
    wordSpans: { text: string; changed: boolean }[] | null,
  ): string {
    if (!line) return "";
    const eolNote = line.no_eol
      ? `<span class="select-none text-amber-600/70 dark:text-amber-500/70" title="No newline at end of file">⏎</span>`
      : "";

    // 1) word-diff spans (paired lines in split view)
    if (wordSpans) {
      const html = wordSpans
        .map(
          (sp) =>
            `<span${sp.changed ? ' class="rounded-sm bg-amber-400/25 dark:bg-amber-300/20"' : ""}>${wsMark(esc(sp.text))}</span>`,
        )
        .join("");
      return html + eolNote;
    }

    // 2) syntax tokens
    if (syntax) {
      const tokens = tokensFor(fc.index, hunkIndex, theme);
      if (tokens && lineIndex != null && tokens[lineIndex]) {
        const html = tokens[lineIndex]
          .map((tk: ThemedTokenLike) => {
            const style = tk.color ? `color:${tk.color};${fontStyleCss(tk.fontStyle)}` : "";
            return `<span${style ? ` style="${style}"` : ""}>${wsMark(esc(tk.content))}</span>`;
          })
          .join("");
        return html + eolNote;
      }
    }

    // 3) plain
    return wsMark(esc(line.content)) + eolNote;
  }

  function lineBg(kind: DiffLine["kind"]): string {
    switch (kind) {
      case "add":
        return "bg-green-500/10";
      case "remove":
        return "bg-red-500/10";
      default:
        return "";
    }
  }
</script>

<div class="flex h-full min-h-0 flex-col">
  <!-- toolbar -->
  <div class="flex items-center gap-1 border-b px-2 py-1.5">
    <div class="flex items-center rounded-md bg-muted/60 p-0.5">
      <button
        class="flex items-center gap-1 rounded px-2 py-1 text-xs {viewMode === 'unified'
          ? 'bg-background text-foreground shadow-sm'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => settings.setDiffViewMode("unified")}
        title={t("diff.unified")}
      >
        <Rows3 class="size-3.5" />{t("diff.unified")}
      </button>
      <button
        class="flex items-center gap-1 rounded px-2 py-1 text-xs {viewMode === 'split'
          ? 'bg-background text-foreground shadow-sm'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => settings.setDiffViewMode("split")}
        title={t("diff.split")}
      >
        <Columns2 class="size-3.5" />{t("diff.split")}
      </button>
    </div>

    <button
      class="flex items-center gap-1 rounded px-2 py-1 text-xs {showWhitespace
        ? 'bg-muted text-foreground'
        : 'text-muted-foreground hover:text-foreground'}"
      onclick={() => settings.setDiffShowWhitespace(!showWhitespace)}
      title={t("diff.showWhitespace")}
    >
      <WholeWord class="size-3.5" />{t("diff.whitespace")}
    </button>

    <button
      class="rounded px-2 py-1 text-xs {ignoreWhitespace
        ? 'bg-muted text-foreground'
        : 'text-muted-foreground hover:text-foreground'}"
      onclick={() => onignorewschange(!ignoreWhitespace)}
      title={t("diff.ignoreWhitespace")}
    >
      {t("diff.ignoreWsShort")}
    </button>

    <button
      class="flex items-center gap-1 rounded px-2 py-1 text-xs {syntax
        ? 'bg-muted text-foreground'
        : 'text-muted-foreground hover:text-foreground'}"
      onclick={() => settings.setDiffSyntax(!syntax)}
      title={t("diff.syntax")}
    >
      <Sparkles class="size-3.5" />{t("diff.syntaxShort")}
    </button>
  </div>

  <!-- body -->
  <div
    class="diff-body min-h-0 flex-1 overflow-auto"
    bind:clientHeight={viewportH}
    onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}
  >
    {#if loading}
      <div class="p-4 text-sm text-muted-foreground">{t("diff.loading")}</div>
    {:else if !model || model.files.length === 0}
      <EmptyState
        animate={false}
        icon={FileText}
        title={t("workspace.noFileSelected")}
        hint={t("workspace.diffHint")}
      />
    {:else}
      <div class="relative" style="height: {totalH}px; width: {contentW}px; min-width: 100%">
        {#each visible as v (rowKey(v.row, v.i))}
          <div class="absolute left-0 right-0" style="top: {v.top}px; height: {v.h}px">
            {@render renderRow(v.row)}
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>

{#snippet renderRow(row: Row)}
  {#if row.t === "file-header"}
    <div class="flex items-center gap-2 border-b bg-muted/40 px-3 py-1 text-xs">
      <span class="min-w-0 flex-1 truncate font-medium" title={row.file.path}>
        {#if row.file.file.old_path && row.file.file.old_path !== row.file.path}
          <span class="text-red-600/80 line-through dark:text-red-400/80">{row.file.file.old_path}</span>
          <span class="mx-1">→</span>
          <span class="text-green-700 dark:text-green-400">{row.file.path}</span>
          {#if row.file.file.similarity != null}
            <span class="ml-1 text-muted-foreground">{row.file.file.similarity}%</span>
          {/if}
        {:else}
          {row.file.path}
        {/if}
      </span>
      {#if row.file.file.hunks.length > 0 && !row.file.file.binary}
        <button
          class="rounded px-1.5 py-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
          onclick={() => onexpand(row.file.path, "all")}
        >{t("diff.expandAll")}</button>
      {/if}
      {#if ops.primary && row.file.lineOpsAllowed}
        {@const sel = selectionsOf(row.file)}
        {#if sel.length > 0}
          <button
            class="rounded bg-green-600/90 px-1.5 py-0.5 font-medium text-white hover:bg-green-600"
            onclick={() => selectedOp(ops.primary!, row.file)}
          >{t(ops.primary === "stage" ? "diff.stageSelected" : "diff.unstageSelected", { n: sel.length })}</button>
          {#if ops.secondary}
            <button
              class="rounded bg-red-600/90 px-1.5 py-0.5 font-medium text-white hover:bg-red-600"
              onclick={() => selectedOp(ops.secondary!, row.file)}
            >{t("diff.discardSelected", { n: sel.length })}</button>
          {/if}
        {/if}
      {/if}
      {#if row.adds > 0}
        <span class="font-mono text-green-600 dark:text-green-400">+{row.adds}</span>
      {/if}
      {#if row.dels > 0}
        <span class="font-mono text-red-600 dark:text-red-400">-{row.dels}</span>
      {/if}
    </div>
  {:else if row.t === "binary"}
    <div class="flex h-full items-center justify-center text-xs text-muted-foreground">
      {t("diff.binary")} — {t("diff.binaryHint")}
    </div>
  {:else if row.t === "image"}
    <div class="h-full border-b bg-muted/20">
      <ImageDiff
        repoId={repoId}
        path={row.file.path}
        source={model?.source ?? "worktree"}
        oldRev={model?.old_revision ?? null}
        newRev={model?.new_revision ?? null}
      />
    </div>
  {:else if row.t === "hunk-header"}
    <div
      class="flex h-full items-center gap-2 border-y border-border/60 bg-cyan-500/5 px-3 font-mono text-[11px] text-cyan-700 dark:text-cyan-300"
    >
      <span class="min-w-0 flex-1 truncate">
        @@ -{row.hunk.old_start},{row.hunk.old_count} +{row.hunk.new_start},{row.hunk.new_count}
        @@ {row.hunk.header}
      </span>
      {#if ops.primary && row.canStage}
        <button
          class="rounded px-1.5 py-0 text-[11px] text-green-700 hover:bg-green-500/10 dark:text-green-400"
          onclick={() => hunkOp(ops.primary!, row.file, row.hunkIndex)}
        >{t(ops.primary === "stage" ? "diff.stageHunk" : "diff.unstageHunk")}</button>
      {/if}
      {#if ops.secondary && row.canStage}
        <button
          class="rounded px-1.5 py-0 text-[11px] text-red-700 hover:bg-red-500/10 dark:text-red-400"
          onclick={() => hunkOp(ops.secondary!, row.file, row.hunkIndex)}
        >{t("diff.discardHunk")}</button>
      {/if}
    </div>
  {:else if row.t === "expand"}
    <button
      class="block h-full w-full px-3 text-center text-[11px] text-muted-foreground hover:bg-muted hover:text-foreground"
      onclick={() => doExpand(row.file, row.dir)}
    >{t("diff.expandContext", { arrow: row.dir === "up" ? "↑" : "↓", n: 10 })}</button>
  {:else if row.t === "collapse"}
    <button
      class="block h-full w-full px-3 text-center text-[11px] text-muted-foreground hover:bg-muted hover:text-foreground"
      onclick={() => toggleExpand(row.runKey)}
    >⋯ {t("diff.collapsedLines", { n: row.hidden })} ⋯</button>
  {:else if row.t === "line"}
    {@const set = selections.get(row.file.path)}
    {@const selected = set?.has(`${row.hunkIndex}:${row.lineIndex}`) ?? false}
    {@const clickable =
      (ops.primary || ops.secondary) && row.file.lineOpsAllowed &&
      (row.line.kind === "add" || row.line.kind === "remove")}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div
      class="flex h-full font-mono text-xs leading-5 {lineBg(row.line.kind)} {selected
        ? 'ring-1 ring-inset ring-blue-400/60'
        : ''} {clickable ? 'cursor-pointer' : ''}"
      onclick={(e) => onLineClick(row.file, row.hunkIndex, row.lineIndex, row.line, e)}
    >
      <span
        class="w-12 shrink-0 select-none border-r border-border/60 pr-1.5 text-right text-muted-foreground/60"
      >{row.line.left_no ?? ""}</span>
      <span
        class="w-12 shrink-0 select-none border-r border-border/60 pr-1.5 text-right text-muted-foreground/60"
      >{row.line.right_no ?? ""}</span>
      <span class="min-w-0 whitespace-pre pl-2">
        {@html renderContent(row.file, row.hunkIndex, row.lineIndex, row.line, null)}
      </span>
    </div>
  {:else if row.t === "pair"}
    {@const set = selections.get(row.file.path)}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="flex h-full font-mono text-xs leading-5">
      <div
        class="min-w-0 flex-1 {lineBg(row.left?.kind ?? 'context')} {row.left &&
        (set?.has(`${row.hunkIndex}:${row.leftIndex}`) ?? false)
          ? 'ring-1 ring-inset ring-blue-400/60'
          : ''} {(ops.primary || ops.secondary) && row.file.lineOpsAllowed && row.left ? 'cursor-pointer' : ''}"
        onclick={(e) =>
          row.left &&
          row.leftIndex != null &&
          onLineClick(row.file, row.hunkIndex, row.leftIndex, row.left, e)}
      >
        <span class="ml-1 inline-block w-10 select-none text-right text-muted-foreground/60"
          >{row.left?.left_no ?? ""}</span
        >
        <span class="whitespace-pre pl-1">
          {@html renderContent(row.file, row.hunkIndex, row.leftIndex, row.left, row.wordDiff?.[0] ?? null)}
        </span>
      </div>
      <div
        class="min-w-0 flex-1 border-l border-border/60 {lineBg(row.right?.kind ?? 'context')} {row.right &&
        (set?.has(`${row.hunkIndex}:${row.rightIndex}`) ?? false)
          ? 'ring-1 ring-inset ring-blue-400/60'
          : ''} {(ops.primary || ops.secondary) && row.file.lineOpsAllowed && row.right ? 'cursor-pointer' : ''}"
        onclick={(e) =>
          row.right &&
          row.rightIndex != null &&
          onLineClick(row.file, row.hunkIndex, row.rightIndex, row.right, e)}
      >
        <span class="ml-1 inline-block w-10 select-none text-right text-muted-foreground/60"
          >{row.right?.right_no ?? ""}</span
        >
        <span class="whitespace-pre pl-1">
          {@html renderContent(row.file, row.hunkIndex, row.rightIndex, row.right, row.wordDiff?.[1] ?? null)}
        </span>
      </div>
    </div>
  {/if}
{/snippet}
