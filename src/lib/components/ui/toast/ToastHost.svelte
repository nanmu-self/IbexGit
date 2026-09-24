<script lang="ts" module>
  import { fly } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { browser } from "$app/environment";

  // `tw-animate-css` `animate-out` is a CSS animation and cannot play at DOM
  // removal time, so the exit is a Svelte transition: same 8px edge in and
  // out, instead of vanishing in place. Reduced motion → opacity-led.
  const reduceMotion =
    browser && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const toastFly = reduceMotion
    ? { y: 0, duration: 120, easing: cubicOut }
    : { y: 8, duration: 200, easing: cubicOut };
</script>

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
    success: "text-success dark:text-success",
    info: "text-info0",
    warning: "text-warning0",
  };
</script>

<div class="pointer-events-none fixed right-4 bottom-4 z-[100] flex flex-col gap-2">
  {#each $toasts as toast (toast.id)}
    {@const Icon = ICONS[toast.type] ?? Info}
    <div
      class="pointer-events-auto flex max-w-md min-w-[300px] items-start gap-2.5 rounded-lg border bg-popover p-3 text-popover-foreground shadow-lg"
      transition:fly={toastFly}
      role="alert"
    >
      <Icon class="mt-0.5 size-4 shrink-0 {COLORS[toast.type] ?? COLORS.info}" />
      <div class="min-w-0 flex-1">
        <div class="text-sm break-words">{toast.message}</div>
        {#if toast.detail}
          <div class="mt-0.5 text-xs break-words text-muted-foreground">{toast.detail}</div>
        {/if}
        {#if toast.action}
          <button
            type="button"
            class="mt-1.5 rounded-md border border-border/60 px-2 py-0.5 text-xs font-medium transition-colors duration-[120ms] hover:bg-accent"
            onclick={() => {
              toast.action?.run();
              removeToast(toast.id);
            }}
          >
            {toast.action.label}
          </button>
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
