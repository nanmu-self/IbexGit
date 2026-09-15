<script lang="ts">
  import type { Component, Snippet } from "svelte";

  let {
    icon = null,
    title,
    hint = null,
    action = null,
    compact = false,
  }: {
    icon?: Component<{ class?: string }> | null;
    title: string;
    hint?: string | null;
    action?: Snippet | null;
    compact?: boolean;
  } = $props();
</script>

<div
  class={compact
    ? "p-3"
    : "flex h-full flex-col items-center justify-center gap-2 p-6 text-center"}
>
  {#if icon}
    {@const Icon = icon}
    <Icon class={compact ? "size-4" : "size-10 text-muted-foreground/40"} />
  {/if}
  <div class={`${compact ? "text-xs" : "text-sm font-medium"} text-muted-foreground`}>
    {title}
  </div>
  {#if hint}
    <div class="max-w-xs text-xs text-muted-foreground/70">{hint}</div>
  {/if}
  {#if action}
    {@render action()}
  {/if}
</div>
