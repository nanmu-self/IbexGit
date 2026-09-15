<script lang="ts">
  import { toasts, removeToast } from "$lib/stores/toast";
  import CircleX from "@lucide/svelte/icons/circle-x";
  import CircleCheck from "@lucide/svelte/icons/circle-check";
  import Info from "@lucide/svelte/icons/info";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
  import X from "@lucide/svelte/icons/x";

  const ICONS = {
    error: CircleX,
    success: CircleCheck,
    info: Info,
    warning: TriangleAlert,
  } as const;

  const COLORS: Record<string, string> = {
    error: "text-destructive",
    success: "text-green-600 dark:text-green-400",
    info: "text-blue-500",
    warning: "text-amber-500",
  };
</script>

<div class="pointer-events-none fixed right-4 bottom-4 z-50 flex flex-col gap-2">
  {#each $toasts as toast (toast.id)}
    {@const Icon = ICONS[toast.type] ?? Info}
    <div
      class="pointer-events-auto flex max-w-md min-w-[300px] items-start gap-2.5 rounded-lg border bg-popover p-3 text-popover-foreground shadow-lg animate-in fade-in slide-in-from-bottom-2"
      role="alert"
    >
      <Icon class="mt-0.5 size-4 shrink-0 {COLORS[toast.type] ?? COLORS.info}" />
      <div class="min-w-0 flex-1">
        <div class="text-sm break-words">{toast.message}</div>
        {#if toast.detail}
          <div class="mt-0.5 text-xs break-words text-muted-foreground">{toast.detail}</div>
        {/if}
      </div>
      <button
        type="button"
        class="rounded p-0.5 text-muted-foreground/60 hover:bg-muted hover:text-foreground"
        onclick={() => removeToast(toast.id)}
      >
        <X class="size-3.5" />
        <span class="sr-only">Close</span>
      </button>
    </div>
  {/each}
</div>
