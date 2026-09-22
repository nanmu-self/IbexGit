<script lang="ts" module>
  import { fly } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { browser } from "$app/environment";

  // Subtree expand/collapse bridge: children drop in from the parent row
  // (4px) and fade, 150ms. Reduced motion → 80ms, opacity-led (y: 0).
  const reduceMotion =
    browser && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  const subtreeFly = reduceMotion
    ? { y: 0, duration: 80, easing: cubicOut }
    : { y: -4, duration: 150, easing: cubicOut };
</script>

<script lang="ts">
  import Self from "./Tree.svelte";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import type { TreeNode, TreeProps } from "./types";

  let {
    nodes,
    expanded,
    activeId = null,
    onToggle,
    onActivate,
    onActivateSecondary,
    onLeafContext,
    depth = 0,
  }: TreeProps = $props();

  function isOpen(node: TreeNode): boolean {
    return expanded.has(node.id);
  }

  function handleClick(node: TreeNode): void {
    if (node.muted) return;
    if (node.children?.length) {
      onToggle?.(node, !isOpen(node));
    }
    onActivate?.(node);
  }
</script>

{#each nodes as node (node.id)}
  {@const open = isOpen(node)}
  <button
    type="button"
    class="group flex w-full items-center gap-1.5 rounded-md py-1 pr-2 text-left text-[13px] leading-5 {node.muted
      ? 'cursor-default text-muted-foreground/50'
      : 'cursor-pointer hover:bg-accent/60'} {node.current
      ? 'bg-accent font-medium text-accent-foreground'
      : ''}"
    style="padding-left: {depth * 12 + 6}px"
    onclick={() => handleClick(node)}
    ondblclick={() => !node.muted && onActivateSecondary?.(node)}
    oncontextmenu={onLeafContext
      ? (e) => {
          e.preventDefault();
          onLeafContext(node, e);
        }
      : undefined}
  >
    {#if node.children?.length}
      <ChevronRight
        class="size-3.5 shrink-0 text-muted-foreground transition-transform duration-100 {open
          ? 'rotate-90'
          : ''}"
      />
    {:else}
      <span class="w-3.5 shrink-0"></span>
    {/if}
    {#if node.icon}
      {@const Icon = node.icon}
      <Icon class="size-4 shrink-0 {node.current ? '' : 'text-muted-foreground'}" />
    {/if}
    <span class="min-w-0 flex-1 truncate" title={node.label}>{node.label}</span>
    {#if node.badge !== undefined && node.badge !== null && node.badge !== ""}
      <span
        class="ml-auto shrink-0 rounded-full bg-muted px-1.5 text-[11px] leading-4 text-muted-foreground"
      >
        {node.badge}
      </span>
    {/if}
    {#if node.trailing}
      <span class="shrink-0 text-[11px] tabular-nums text-muted-foreground">
        {node.trailing}
      </span>
    {/if}
  </button>
  {#if open && node.children?.length}
    <div transition:fly={subtreeFly}>
      <Self
        nodes={node.children}
        {expanded}
        {activeId}
        {onToggle}
        {onActivate}
        {onActivateSecondary}
        {onLeafContext}
        depth={depth + 1}
      />
    </div>
  {/if}
{/each}
