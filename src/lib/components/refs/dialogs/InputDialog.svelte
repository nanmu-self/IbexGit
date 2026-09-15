<script lang="ts">
  // Generic single-value dialog (P6): rename branch, set upstream, remote
  // URL, new branch name… Validation runs on submit; Enter submits.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { t } from "$lib/i18n";

  let {
    open = $bindable(false),
    title,
    description = "",
    label = "",
    value = $bindable(""),
    placeholder = "",
    confirmLabel = "",
    busy = false,
    validate,
    onsubmit,
  }: {
    open?: boolean;
    title: string;
    description?: string;
    label?: string;
    value?: string;
    placeholder?: string;
    confirmLabel?: string;
    busy?: boolean;
    validate?: (v: string) => string | null;
    onsubmit: (v: string) => void;
  } = $props();

  let error = $state<string | null>(null);

  // Reset the error each time the dialog opens.
  $effect(() => {
    if (open) error = null;
  });

  function submit(e: SubmitEvent): void {
    e.preventDefault();
    const v = value.trim();
    if (!v) {
      error = t("refs.errors.required");
      return;
    }
    const problem = validate?.(v) ?? null;
    if (problem) {
      error = problem;
      return;
    }
    onsubmit(v);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>{title}</Dialog.Title>
      {#if description}
        <Dialog.Description>{description}</Dialog.Description>
      {/if}
    </Dialog.Header>
    <form class="space-y-3" onsubmit={submit}>
      {#if label}
        <label class="text-xs text-muted-foreground" for="input-dialog-field">{label}</label>
      {/if}
      <Input
        id="input-dialog-field"
        bind:value
        {placeholder}
        class="font-mono text-[13px]"
        autofocus
      />
      {#if error}
        <p class="text-xs text-destructive">{error}</p>
      {/if}
      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={busy}>{confirmLabel || t("common.ok")}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
