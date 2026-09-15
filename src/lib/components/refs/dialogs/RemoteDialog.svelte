<script lang="ts">
  // Add / edit remote dialog (P6): name + URL.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { t } from "$lib/i18n";

  let {
    open = $bindable(false),
    /** Existing remote name when editing; empty when adding. */
    editName = "",
    initialUrl = "",
    busy = false,
    onsubmit,
  }: {
    open?: boolean;
    editName?: string;
    initialUrl?: string;
    busy?: boolean;
    onsubmit: (name: string, url: string) => Promise<void>;
  } = $props();

  let name = $state("");
  let url = $state("");

  $effect(() => {
    if (open) {
      name = editName;
      url = initialUrl;
    }
  });

  const editing = $derived(editName !== "");

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    try {
      await onsubmit(name.trim(), url.trim());
      open = false;
    } catch {
      /* toast surfaced */
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>
        {editing ? t("refs.remote.editTitle", { name: editName }) : t("refs.remote.addTitle")}
      </Dialog.Title>
      <Dialog.Description>{t("refs.remote.desc")}</Dialog.Description>
    </Dialog.Header>
    <form class="space-y-3" onsubmit={submit}>
      <label class="block space-y-1">
        <span class="text-xs text-muted-foreground">{t("refs.remote.name")}</span>
        <Input bind:value={name} placeholder="origin" class="font-mono text-[13px]" disabled={editing} autofocus />
      </label>
      <label class="block space-y-1">
        <span class="text-xs text-muted-foreground">{t("refs.remote.url")}</span>
        <Input bind:value={url} placeholder="https://github.com/user/repo.git" class="font-mono text-[13px]" />
      </label>
      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={busy || !name.trim() || !url.trim()}>
          {editing ? t("common.ok") : t("refs.remote.add")}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
