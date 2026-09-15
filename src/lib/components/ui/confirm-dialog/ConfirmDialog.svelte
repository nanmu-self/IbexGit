<script lang="ts">
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { t } from "$lib/i18n";
  import type { Snippet } from "svelte";

  /**
   * Generic confirmation dialog (P3): used by discard, ignore, recovery
   * restore — anywhere a destructive or double-check action needs a gate.
   * `children` renders optional body content between header and footer.
   */
  let {
    open = $bindable(false),
    title,
    description = "",
    confirmLabel,
    destructive = false,
    onconfirm,
    children,
  }: {
    open?: boolean;
    title: string;
    description?: string;
    confirmLabel: string;
    destructive?: boolean;
    onconfirm: () => void;
    children?: Snippet;
  } = $props();

  function confirm(): void {
    open = false;
    onconfirm();
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>{title}</Dialog.Title>
      {#if description}
        <Dialog.Description>{description}</Dialog.Description>
      {/if}
    </Dialog.Header>
    {#if children}
      {@render children()}
    {/if}
    <Dialog.Footer>
      <Button variant="ghost" size="sm" onclick={() => (open = false)}>
        {t("common.cancel")}
      </Button>
      <Button variant={destructive ? "destructive" : "default"} size="sm" onclick={confirm}>
        {confirmLabel}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
