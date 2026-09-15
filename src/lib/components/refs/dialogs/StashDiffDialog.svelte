<script lang="ts">
  // Stash diff viewer (P6): shows the content of one stash entry via the
  // DiffModel pipeline (source=stash, read-only).
  import * as Dialog from "$lib/components/ui/dialog";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { t } from "$lib/i18n";
  import { git, normalizeError, type DiffModel, type StashEntry } from "$lib/git";

  let {
    open = $bindable(false),
    entry = null,
    repoId,
  }: {
    open?: boolean;
    entry?: StashEntry | null;
    repoId: string | null;
  } = $props();

  let model = $state<DiffModel | null>(null);
  let loading = $state(false);

  $effect(() => {
    const e = entry;
    if (!open || !e || !repoId) {
      model = null;
      return;
    }
    loading = true;
    // Stash content = stash commit vs its first parent.
    git
      .diff(repoId, "stash", `stash@{${e.index}}^`, `stash@{${e.index}}`)
      .then((m) => (model = m))
      .catch((err) => {
        normalizeError(err);
        model = null;
      })
      .finally(() => (loading = false));
  });
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="flex h-[80vh] max-w-3xl flex-col">
    <Dialog.Header>
      <Dialog.Title>{t("refs.stashDiff.title")}</Dialog.Title>
      <Dialog.Description>
        {entry ? `stash@{${entry.index}} · ${entry.message}` : ""}
      </Dialog.Description>
    </Dialog.Header>
    <div class="min-h-0 flex-1 overflow-hidden rounded-md border">
      <DiffViewer
        {model}
        {loading}
        {repoId}
        ignoreWhitespace={false}
        onlineop={() => {}}
        onexpand={() => {}}
        onignorewschange={() => {}}
      />
    </div>
  </Dialog.Content>
</Dialog.Root>
