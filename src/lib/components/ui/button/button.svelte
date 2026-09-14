<script lang="ts">
  import { buttonVariants } from "./button.js";
  import type { HTMLButtonAttributes } from "svelte/elements";
  import { cn } from "$lib/utils.js";

  type ButtonProps = HTMLButtonAttributes & {
    variant?: "default" | "destructive" | "outline" | "secondary" | "ghost" | "link";
    size?: "default" | "sm" | "lg" | "icon";
  };

  let {
    class: className,
    variant = "default",
    size = "default",
    children,
    ...restProps
  }: ButtonProps = $props();
</script>

<button
  class={cn(buttonVariants({ variant, size, class: className }))}
  {...restProps}
>
  {@render children?.()}
</button>
