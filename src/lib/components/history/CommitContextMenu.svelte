<script lang="ts">
  // Commit context menu (P5 收尾增补): checkout (detached), revert, undo
  // (= reset current branch to this commit), create branch/tag from the
  // commit, cherry-pick, copy SHA / message. Reword / drop belong to the
  // interactive-rebase Backlog and are deliberately absent.
  // Hand-rolled like FileContextMenu — outside mousedown + Escape close,
  // viewport-clamped fixed positioning.
  import { fade } from "svelte/transition";
  import { t } from "$lib/i18n";
  import type { GraphRow } from "$lib/git";
  import CircleDot from "@lucide/svelte/icons/circle-dot";
  import RotateCcw from "@lucide/svelte/icons/rotate-ccw";
  import Undo2 from "@lucide/svelte/icons/undo-2";
  import GitBranchPlus from "@lucide/svelte/icons/git-branch-plus";
  import TagIcon from "@lucide/svelte/icons/tag";
  import Cherry from "@lucide/svelte/icons/cherry";
  import Copy from "@lucide/svelte/icons/copy";
  import ClipboardCopy from "@lucide/svelte/icons/clipboard-copy";

  export interface CommitContextTarget {
    row: GraphRow;
    x: number;
    y: number;
  }

  let {
    target,
    onclose,
    oncheckout,
    onrevert,
    onundo,
    onbranch,
    ontag,
    oncherrypick,
  }: {
    target: CommitContextTarget | null;
    onclose: () => void;
    /** Checkout detached at this commit. */
    oncheckout: (row: GraphRow) => void;
    /** Revert this commit (opens the preview dialog with a single entry). */
    onrevert: (row: GraphRow) => void;
    /** Reset current branch to this commit (prefilled ResetDialog). */
    onundo: (row: GraphRow) => void;
    /** Create a branch pointing at this commit. */
    onbranch: (row: GraphRow) => void;
    /** Create a tag on this commit. */
    ontag: (row: GraphRow) => void;
    /** Cherry-pick this commit (opens the preview dialog). */
    oncherrypick: (row: GraphRow) => void;
  } = $props();

  let panel = $state<HTMLDivElement | null>(null);

  $effect(() => {
    if (!target) return;
    const ondown = (e: MouseEvent): void => {
      if (panel && !panel.contains(e.target as Node)) onclose();
    };
    const onescape = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onclose();
    };
    window.addEventListener("mousedown", ondown, true);
    window.addEventListener("keydown", onescape, true);
    return () => {
      window.removeEventListener("mousedown", ondown, true);
      window.removeEventListener("keydown", onescape, true);
    };
  });

  // Keep the menu inside the viewport.
  function style(x: number, y: number): string {
    const W = 224;
    const H = 280;
    const nx = Math.min(x, window.innerWidth - W - 8);
    const ny = Math.min(y, window.innerHeight - H - 8);
    return `left:${nx}px;top:${ny}px`;
  }

  function run(fn: (row: GraphRow) => void): () => void {
    return () => {
      if (target) fn(target.row);
      onclose();
    };
  }

  function copySha(row: GraphRow): void {
    navigator.clipboard?.writeText(row.commit.hash).catch(() => {});
  }

  function copyMessage(row: GraphRow): void {
    navigator.clipboard?.writeText(row.commit.message).catch(() => {});
  }
</script>

{#if target}
  <div
    bind:this={panel}
    class="fixed z-50 min-w-52 rounded-md border bg-popover p-1 text-popover-foreground shadow-md"
    transition:fade={{ duration: 100 }}
    style={style(target.x, target.y)}
  >
    <button type="button" class="menu-item" onclick={run(oncheckout)}>
      <CircleDot class="size-3.5" /> {t("history.ctx.checkout")}
    </button>
    <button type="button" class="menu-item" onclick={run(onrevert)}>
      <RotateCcw class="size-3.5" /> {t("history.ctx.revert")}
    </button>
    <button type="button" class="menu-item" onclick={run(onundo)}>
      <Undo2 class="size-3.5" /> {t("history.ctx.undo")}
    </button>
    <div class="my-1 h-px bg-border"></div>
    <button type="button" class="menu-item" onclick={run(onbranch)}>
      <GitBranchPlus class="size-3.5" /> {t("history.ctx.branch")}
    </button>
    <button type="button" class="menu-item" onclick={run(ontag)}>
      <TagIcon class="size-3.5" /> {t("history.ctx.tag")}
    </button>
    <button type="button" class="menu-item" onclick={run(oncherrypick)}>
      <Cherry class="size-3.5" /> {t("history.ctx.cherryPick")}
    </button>
    <div class="my-1 h-px bg-border"></div>
    <button type="button" class="menu-item" onclick={run(copySha)}>
      <Copy class="size-3.5" /> {t("history.ctx.copySha")}
    </button>
    <button type="button" class="menu-item" onclick={run(copyMessage)}>
      <ClipboardCopy class="size-3.5" /> {t("history.ctx.copyMessage")}
    </button>
  </div>
{/if}

<style>
  .menu-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    border-radius: calc(var(--radius-md) - 2px);
    padding: 0.3rem 0.5rem;
    font-size: 13px;
    text-align: left;
  }
  .menu-item:hover {
    background: var(--color-accent);
  }
</style>
