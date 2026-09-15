<script lang="ts">
  import { t } from "$lib/i18n";
  import History from "@lucide/svelte/icons/history";
  import TextSelect from "@lucide/svelte/icons/text-select";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import Copy from "@lucide/svelte/icons/copy";
  import EyeOff from "@lucide/svelte/icons/eye-off";

  export interface ContextTarget {
    path: string;
    untracked: boolean;
    /** Absolute worktree path (repo root + relative path). */
    absPath: string;
    x: number;
    y: number;
  }

  let {
    target,
    onclose,
    onhistory,
    onblame,
    onreveal,
    oncopypath,
    onignore,
  }: {
    target: ContextTarget | null;
    onclose: () => void;
    onhistory: (t: ContextTarget) => void;
    onblame: (t: ContextTarget) => void;
    onreveal: (t: ContextTarget) => void;
    oncopypath: (t: ContextTarget) => void;
    onignore: (t: ContextTarget) => void;
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
    const W = 210;
    const H = 200;
    const nx = Math.min(x, window.innerWidth - W - 8);
    const ny = Math.min(y, window.innerHeight - H - 8);
    return `left:${nx}px;top:${ny}px`;
  }

  function run(fn: (t: ContextTarget) => void): () => void {
    return () => {
      if (target) fn(target);
      onclose();
    };
  }
</script>

{#if target}
  <div
    bind:this={panel}
    class="fixed z-50 min-w-52 rounded-md border bg-popover p-1 text-popover-foreground shadow-md animate-in fade-in zoom-in-95 duration-100"
    style={style(target.x, target.y)}
  >
    <button type="button" class="menu-item" onclick={run(onhistory)}>
      <History class="size-3.5" /> {t("workspace.ctx.history")}
    </button>
    <button type="button" class="menu-item" onclick={run(onblame)}>
      <TextSelect class="size-3.5" /> {t("workspace.ctx.blame")}
    </button>
    <div class="my-1 h-px bg-border"></div>
    <button type="button" class="menu-item" onclick={run(onreveal)}>
      <FolderOpen class="size-3.5" /> {t("workspace.ctx.reveal")}
    </button>
    <button type="button" class="menu-item" onclick={run(oncopypath)}>
      <Copy class="size-3.5" /> {t("workspace.ctx.copyPath")}
    </button>
    {#if target.untracked}
      <div class="my-1 h-px bg-border"></div>
      <button type="button" class="menu-item" onclick={run(onignore)}>
        <EyeOff class="size-3.5" /> {t("workspace.ctx.ignore")}
      </button>
    {/if}
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
