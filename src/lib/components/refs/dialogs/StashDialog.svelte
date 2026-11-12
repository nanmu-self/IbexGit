<script lang="ts">
  // Stash push dialog (贮藏本地变更): scope mode selector + optional
  // message + untracked inclusion, mirroring the TortoiseGit-style layout.
  // - mode "all": worktree + index tracked changes (default)
  // - mode "staged": only staged content; unstaged stay (git --staged,
  //   incompatible with -u, so the untracked checkbox is disabled)
  // - mode "unstaged_only": only unstaged changes; the index stays intact
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import Textarea from "./Textarea.svelte";
  import { t } from "$lib/i18n";
  import type { StashPushMode } from "$lib/git/bindings";
  import { fly } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  let {
    open = $bindable(false),
    busy = false,
    onsubmit,
  }: {
    open?: boolean;
    busy?: boolean;
    onsubmit: (
      mode: StashPushMode,
      message: string | null,
      includeUntracked: boolean
    ) => void;
  } = $props();

  const MODES: StashPushMode[] = ["all", "staged", "unstaged_only"];
  const MODE_LABEL: Record<StashPushMode, string> = {
    all: "refs.stash.modeAll",
    staged: "refs.stash.modeStaged",
    unstaged_only: "refs.stash.modeUnstaged",
  };
  const MODE_DESC: Record<StashPushMode, string> = {
    all: "refs.stash.modeAllDesc",
    staged: "refs.stash.modeStagedDesc",
    unstaged_only: "refs.stash.modeUnstagedDesc",
  };

  let mode = $state<StashPushMode>("all");
  let message = $state("");
  let includeUntracked = $state(true);

  $effect(() => {
    if (open) {
      mode = "all";
      message = "";
      includeUntracked = true;
    }
  });

  function submit(e: SubmitEvent): void {
    e.preventDefault();
    const msg = message.trim();
    // `git stash push --staged` refuses `-u`; never send the flag in that mode.
    onsubmit(mode, msg ? msg : null, mode === "staged" ? false : includeUntracked);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>{t("refs.stash.pushTitle")}</Dialog.Title>
      <Dialog.Description>{t("refs.stash.dialogDesc")}</Dialog.Description>
    </Dialog.Header>
    <form class="space-y-3" onsubmit={submit}>
      <div class="flex items-center gap-3">
        <label for="stash-mode" class="w-14 shrink-0 text-right text-sm text-muted-foreground">
          {t("refs.stash.mode")}
        </label>
        <select
          id="stash-mode"
          bind:value={mode}
          class="h-9 min-w-0 flex-1 rounded-md border bg-background px-2 text-[13px]"
        >
          {#each MODES as m (m)}
            <option value={m}>{t(MODE_LABEL[m])}</option>
          {/each}
        </select>
      </div>
      <p class="pl-17 text-xs text-muted-foreground">
        {#key mode}
          <span
            in:fly={{ y: 6, duration: 160, easing: cubicOut }}
            class="block"
          >
            {t(MODE_DESC[mode])}
          </span>
        {/key}
      </p>

      <div class="flex items-start gap-3">
        <label
          for="stash-message"
          class="w-14 shrink-0 pt-2 text-right text-sm text-muted-foreground"
        >
          {t("refs.stash.messageLabel")}
        </label>
        <div class="min-w-0 flex-1">
          <Textarea
            id="stash-message"
            bind:value={message}
            placeholder={t("refs.stash.messagePlaceholder")}
            rows={4}
          />
        </div>
      </div>

      <div class="flex items-center gap-2 pl-17">
        <Checkbox
          bind:checked={includeUntracked}
          disabled={mode === "staged"}
          id="stash-untracked"
          class="transition-opacity duration-150"
        />
        <label
          for="stash-untracked"
          class="text-sm leading-none transition-opacity duration-150 peer-disabled:cursor-not-allowed peer-disabled:opacity-70"
        >
          {t("refs.stash.includeUntracked")}
        </label>
      </div>

      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={busy}>
          {#if busy}
            <LoaderCircle class="size-3.5 animate-spin" data-icon="inline-start" />
          {/if}
          {t("refs.stash.confirm")}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
