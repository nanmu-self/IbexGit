<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { Input } from "$lib/components/ui/input";
  import { t } from "$lib/i18n";
  import GitCommitHorizontal from "@lucide/svelte/icons/git-commit-horizontal";

  let {
    stagedCount,
    repoReady,
    busy = false,
    mode = "normal",
    oncommit,
    onamend = null,
  }: {
    stagedCount: number;
    repoReady: boolean;
    busy?: boolean;
    /** P8: normal | merge | cherry_pick | revert (operation commit). */
    mode?: "normal" | "merge" | "cherry_pick" | "revert";
    oncommit: (message: string, amend: boolean, noVerify: boolean, andPush: boolean) => Promise<void>;
    /** Called when Amend is checked; resolves to the HEAD message (or null). */
    onamend?: (() => Promise<string | null>) | null;
  } = $props();

  let subject = $state("");
  let description = $state("");
  let amend = $state(false);
  let noVerify = $state(false);
  let andPush = $state(false);
  let submitting = $state(false);

  const canCommit = $derived(
    repoReady && !busy && !submitting && subject.trim().length > 0 && (stagedCount > 0 || amend)
  );

  // Checking Amend loads the HEAD message into the editor (PLAN P3).
  $effect(() => {
    if (amend && onamend) {
      onamend().then((msg) => {
        if (msg !== null && msg !== undefined) loadMessage(msg);
      });
    }
  });

  /** Split a full commit message into subject + description (Amend load). */
  export function loadMessage(full: string | null): void {
    if (full === null) return;
    const trimmed = full.trimEnd();
    const nl = trimmed.indexOf("\n");
    if (nl === -1) {
      subject = trimmed;
      description = "";
    } else {
      subject = trimmed.slice(0, nl);
      description = trimmed.slice(nl + 1).replace(/^\n+/, "").trimEnd();
    }
  }

  function reset(): void {
    subject = "";
    description = "";
    amend = false;
  }

  async function submit(): Promise<void> {
    if (!canCommit) return;
    submitting = true;
    try {
      const desc = description.trim();
      const message = desc ? `${subject.trim()}\n\n${desc}` : subject.trim();
      await oncommit(message, amend, noVerify, andPush);
      reset();
    } finally {
      submitting = false;
    }
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void submit();
    }
  }
</script>

<div class="space-y-2 border-t bg-background p-2.5">
  {#if mode !== "normal"}
    <div class="text-xs text-amber-600 dark:text-amber-400">
      {t(`conflict.commitHint.${mode}`)}
    </div>
  {/if}
  <Input
    placeholder={t("commit.subject")}
    bind:value={subject}
    disabled={!repoReady}
    class="h-8 text-[13px]"
    {onkeydown}
  />
  <textarea
    rows={2}
    placeholder={t("commit.description")}
    bind:value={description}
    disabled={!repoReady}
    {onkeydown}
    class="w-full resize-none rounded-md border border-input bg-transparent px-3 py-2 text-[13px] placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-60"
  ></textarea>
  <div class="flex items-center gap-4 text-xs text-muted-foreground">
    {#if mode === "normal"}
      <label class="flex items-center gap-1.5">
        <Checkbox bind:checked={amend} disabled={!repoReady} />
        {t("commit.amend")}
      </label>
    {/if}
    <label class="flex items-center gap-1.5">
      <Checkbox bind:checked={noVerify} disabled={!repoReady} />
      {t("commit.noVerify")}
    </label>
    {#if mode === "normal"}
      <label class="flex items-center gap-1.5">
        <Checkbox bind:checked={andPush} disabled={!repoReady} />
        {t("commit.andPush")}
      </label>
    {/if}
    <Button
      class="ml-auto h-8 min-w-36 text-xs"
      disabled={!canCommit}
      title={!subject.trim() ? t("commit.needsMessage") : undefined}
      onclick={submit}
    >
      <GitCommitHorizontal class="size-3.5" data-icon="inline-start" />
      {mode === "merge"
        ? t("conflict.commitButton.merge")
        : mode === "cherry_pick"
          ? t("conflict.commitButton.cherry_pick")
          : mode === "revert"
            ? t("conflict.commitButton.revert")
            : amend
              ? t("commit.amendButton")
              : stagedCount > 0
                ? t("commit.buttonN", { n: stagedCount })
                : t("commit.button")}
    </Button>
  </div>
</div>
