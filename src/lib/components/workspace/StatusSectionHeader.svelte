<script lang="ts">
  import { Button } from "$lib/components/ui/button";

  /** One batch action rendered at the section header's right edge. */
  export interface SectionAction {
    label: string;
    onclick: () => void;
    /** Destructive styling (discard). */
    danger?: boolean;
  }

  let {
    title,
    count,
    tone = "muted",
    actions = [],
  }: {
    title: string;
    count: number;
    /** `red` = conflict section accent; `muted` = neutral. */
    tone?: "red" | "muted";
    actions?: SectionAction[];
  } = $props();
</script>

<div
  class="flex items-center gap-2 px-3 py-1.5 text-xs font-medium {tone === 'red'
    ? 'text-red-500'
    : 'text-muted-foreground'}"
>
  <span>{title}</span>
  <span
    class="rounded-full {tone === 'red' ? 'bg-red-500/15' : 'bg-muted'} px-1.5 text-[11px] leading-4"
  >
    {count}
  </span>
  {#if actions.length > 0}
    <div class="ml-auto flex items-center gap-1">
      {#each actions as action, i (i)}
        <Button
          variant="ghost"
          size="xs"
          class="text-[11px] {action.danger
            ? 'text-red-500/90 hover:text-red-500'
            : 'text-muted-foreground'}"
          onclick={action.onclick}
        >
          {action.label}
        </Button>
      {/each}
    </div>
  {/if}
</div>
