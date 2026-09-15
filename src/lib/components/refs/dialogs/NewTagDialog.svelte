<script lang="ts">
  // Create a tag (P6): name + optional message (annotated when present) +
  // target revision (defaults to HEAD).
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import Textarea from "./Textarea.svelte";
  import { t } from "$lib/i18n";

  let {
    open = $bindable(false),
    target = "HEAD",
    busy = false,
    oncreate,
  }: {
    open?: boolean;
    target?: string;
    busy?: boolean;
    oncreate: (name: string, message: string | null, target: string) => void;
  } = $props();

  let name = $state("");
  let message = $state("");
  let targetInput = $state("HEAD");

  $effect(() => {
    if (open) {
      name = "";
      message = "";
      targetInput = target;
    }
  });

  function submit(e: SubmitEvent): void {
    e.preventDefault();
    const n = name.trim();
    if (!n) return;
    const msg = message.trim();
    oncreate(n, msg ? msg : null, targetInput.trim() || "HEAD");
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>{t("refs.tagDialog.title")}</Dialog.Title>
      <Dialog.Description>{t("refs.tagDialog.desc")}</Dialog.Description>
    </Dialog.Header>
    <form class="space-y-3" onsubmit={submit}>
      <Input
        bind:value={name}
        placeholder={t("refs.tagDialog.namePlaceholder")}
        class="font-mono text-[13px]"
        autofocus
      />
      <Textarea
        bind:value={message}
        placeholder={t("refs.tagDialog.messagePlaceholder")}
        rows={3}
      />
      <Input
        bind:value={targetInput}
        placeholder={t("refs.tagDialog.targetPlaceholder")}
        class="font-mono text-[13px]"
      />
      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={busy || !name.trim()}>
          {t("refs.tagDialog.create")}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
