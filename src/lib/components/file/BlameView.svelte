<script lang="ts">
  // P9 Blame 视图：逐行展示 提交着色分组（同一提交的连续行同色）、
  // 行号、作者与日期；未提交行（零 sha 伪提交）特殊标记、不可跳转。
  // 点击已提交行 → 跳到该提交中该文件的 diff（fileView 转到 history 页）。
  import { VirtualList } from "$lib/components/ui/virtual-list";
  import { t } from "$lib/i18n";
  import { git, normalizeError, type BlameResult } from "$lib/git";
  import { repos } from "$lib/stores/repos.svelte";
  import TextSelect from "@lucide/svelte/icons/text-select";

  const ROW_H = 22;

  /** Subtle group tints (light+dark friendly); keyed by commit index. */
  const PALETTE = [
    "bg-rose-500/10",
    "bg-warning-surface",
    "bg-success-surface",
    "bg-info-surface",
    "bg-info-surface",
    "bg-teal-500/10",
    "bg-warning-surface",
    "bg-indigo-500/10",
  ];

  let { path, onjump }: { path: string; onjump: (hash: string) => void } = $props();

  let result = $state<BlameResult | null>(null);
  let loading = $state(false);

  // ---- load whenever (repo, path) changes ----
  let reloadSeq = 0;
  $effect(() => {
    const id = repos.activeId;
    const p = path;
    const seq = ++reloadSeq;
    result = null;
    if (!id || !p) return;
    loading = true;
    git
      .blame(id, p)
      .then((r) => {
        if (seq === reloadSeq) result = r;
      })
      .catch((e) => {
        if (seq === reloadSeq) normalizeError(e);
      })
      .finally(() => {
        if (seq === reloadSeq) loading = false;
      });
  });

  const lines = $derived(result?.lines ?? []);
  const commits = $derived(result?.commits ?? []);

  function tint(commitIndex: number): string {
    return PALETTE[commitIndex % PALETTE.length];
  }

  function groupStart(index: number): boolean {
    return index === 0 || lines[index - 1].commit !== lines[index].commit;
  }

  function tooltip(index: number): string {
    const c = commits[lines[index].commit];
    if (!c) return "";
    if (c.uncommitted) return t("blame.uncommitted");
    return `${c.short_hash} ${c.summary} — ${c.author}, ${c.date}`;
  }

  function fmtDate(iso: string): string {
    const d = new Date(iso);
    const now = new Date();
    return d.toDateString() === now.toDateString()
      ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
      : d.toLocaleDateString([], { month: "short", day: "numeric" });
  }

  function click(index: number): void {
    const c = commits[lines[index].commit];
    if (!c || c.uncommitted) return;
    onjump(c.hash);
  }
</script>

<div class="flex min-h-0 flex-1 flex-col">
  {#if loading && !result}
    <div class="p-4 text-sm text-muted-foreground">{t("common.loading")}</div>
  {:else if lines.length === 0}
    <div class="flex flex-1 items-center justify-center p-4 text-center text-sm text-muted-foreground">
      <div>
        <TextSelect class="mx-auto mb-2 size-6 opacity-40" />
        {t("blame.empty")}
      </div>
    </div>
  {:else}
    <VirtualList items={lines} itemHeight={ROW_H} overscan={20} scrollX getKey={(_l, i) => i}>
      {#snippet row(line, index: number)}
        {@const c = commits[line.commit]}
        {@const start = groupStart(index)}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div
          class="flex h-full items-stretch font-mono text-xs leading-[22px] {tint(line.commit)} {start
            ? 'border-t border-border/70'
            : ''} {c && !c.uncommitted ? 'cursor-pointer hover:brightness-95' : ''}"
          style="height: {ROW_H}px"
          title={tooltip(index)}
          onclick={() => click(index)}
        >
          <span class="w-12 shrink-0 select-none border-r border-border/40 pr-2 text-right text-muted-foreground/70">
            {line.final_no}
          </span>
          {#if c}
            <span class="flex w-52 shrink-0 items-center gap-2 border-r border-border/40 px-2">
              {#if c.uncommitted}
                <span
                  class="truncate rounded bg-warning/20 px-1.5 py-0 text-[10px] font-medium text-warning dark:text-warning"
                >
                  {t("blame.uncommitted")}
                </span>
              {:else}
                <span class="shrink-0 font-semibold">{c.short_hash}</span>
                <span class="min-w-0 flex-1 truncate" title={c.author}>{c.author}</span>
                <span class="shrink-0 text-muted-foreground/80">{fmtDate(c.date)}</span>
              {/if}
            </span>
          {/if}
          <span class="min-w-0 whitespace-pre pl-2">{line.content}</span>
        </div>
      {/snippet}
    </VirtualList>
    <div class="flex items-center gap-2 border-t px-3 py-1 text-[11px] text-muted-foreground">
      {t("blame.lineCount", { n: lines.length })}
      · {t("blame.commitCount", { n: commits.filter((c) => !c.uncommitted).length })}
      <span class="ml-auto">{t("blame.jumpHint")}</span>
    </div>
  {/if}
</div>
