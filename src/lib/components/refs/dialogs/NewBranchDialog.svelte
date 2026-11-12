<script lang="ts">
  // Create-branch dialog (创建本地分支): base display + name + dirty-change
  // handling (keep / stash+auto-restore / discard) + force recreate +
  // checkout-after. Matches the TortoiseGit-style layout with the project's
  // shadcn form language.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { Input } from "$lib/components/ui/input";
  import { t } from "$lib/i18n";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  let {
    open = $bindable(false),
    busy = false,
    /** Ref the new branch points at (start point, else the current branch). */
    base = "",
    onsubmit,
  }: {
    open?: boolean;
    busy?: boolean;
    base?: string;
    onsubmit: (
      name: string,
      dirty: "keep" | "stash" | "discard",
      force: boolean,
      checkout: boolean
    ) => void;
  } = $props();

  let name = $state("");
  let dirty = $state<"keep" | "stash" | "discard">("keep");
  let force = $state(false);
  let checkout = $state(true);
  let error = $state<string | null>(null);

  $effect(() => {
    if (open) {
      name = "";
      dirty = "keep";
      force = false;
      checkout = true;
      error = null;
    }
  });

  function submit(e: SubmitEvent): void {
    e.preventDefault();
    const v = name.trim();
    if (!v) {
      error = t("refs.errors.required");
      return;
    }
    if (/[\s~^:?*\[\\]/.test(v)) {
      error = t("refs.errors.badRefName");
      return;
    }
    error = null;
    onsubmit(v, dirty, force, checkout);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>{t("refs.newBranch.title")}</Dialog.Title>
      <Dialog.Description>{t("refs.newBranch.desc")}</Dialog.Description>
    </Dialog.Header>
    <form class="space-y-3" onsubmit={submit}>
      <div class="flex items-center gap-3">
        <span class="w-24 shrink-0 text-right text-sm whitespace-nowrap text-muted-foreground">
          {t("refs.newBranch.base")}
        </span>
        <span class="flex min-w-0 flex-1 items-center gap-1.5 text-[13px]">
          <GitBranch class="size-3.5 shrink-0 text-muted-foreground" />
          <span class="truncate font-mono" title={base}>{base}</span>
        </span>
      </div>

      <div class="flex items-center gap-3">
        <label
          for="new-branch-name"
          class="w-24 shrink-0 text-right text-sm whitespace-nowrap text-muted-foreground"
        >
          {t("refs.newBranch.nameLabel")}
        </label>
        <Input
          id="new-branch-name"
          bind:value={name}
          placeholder="feature/…"
          class="min-w-0 flex-1 font-mono text-[13px]"
          autofocus
        />
      </div>

      <div class="flex items-start gap-3">
        <span class="w-24 shrink-0 pt-1.5 text-right text-sm whitespace-nowrap text-muted-foreground">
          {t("refs.newBranch.dirtyLabel")}
        </span>
        <div class="min-w-0 flex-1 space-y-1.5">
          <label class="flex cursor-pointer items-center gap-2 text-sm">
            <input type="radio" class="mt-0.5" bind:group={dirty} value="keep" />
            <span>{t("refs.newBranch.dirtyKeep")}</span>
          </label>
          <label class="flex cursor-pointer items-center gap-2 text-sm">
            <input type="radio" class="mt-0.5" bind:group={dirty} value="stash" />
            <span>{t("refs.newBranch.dirtyStash")}</span>
          </label>
          <label class="flex cursor-pointer items-center gap-2 text-sm">
            <input type="radio" class="mt-0.5" bind:group={dirty} value="discard" />
            <span>{t("refs.newBranch.dirtyDiscard")}</span>
          </label>
          {#if dirty === "discard"}
            <p class="rounded-md bg-warning-surface px-2.5 py-2 text-xs text-warning dark:text-warning">
              {t("refs.newBranch.dirtyDiscardWarn")}
            </p>
          {/if}
        </div>
      </div>

      <div class="flex items-center gap-2 pl-27">
        <Checkbox bind:checked={force} id="new-branch-force" />
        <label
          for="new-branch-force"
          class="text-sm leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70"
        >
          {t("refs.newBranch.allowReset")}
        </label>
      </div>
      <div class="flex items-center gap-2 pl-27">
        <Checkbox bind:checked={checkout} id="new-branch-checkout" />
        <label
          for="new-branch-checkout"
          class="text-sm leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70"
        >
          {t("refs.newBranch.checkoutAfter")}
        </label>
      </div>

      {#if error}
        <p class="text-xs text-destructive">{error}</p>
      {/if}

      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={busy}>
          {#if busy}
            <LoaderCircle class="size-3.5 animate-spin" data-icon="inline-start" />
          {/if}
          {t("common.ok")}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
