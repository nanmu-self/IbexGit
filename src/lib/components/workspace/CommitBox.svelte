<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { Input } from "$lib/components/ui/input";
  import { t } from "$lib/i18n";
  import GitCommitHorizontal from "@lucide/svelte/icons/git-commit-horizontal";
  import Plus from "@lucide/svelte/icons/plus";
  import X from "@lucide/svelte/icons/x";
  import Users from "@lucide/svelte/icons/users";

  let {
    stagedCount,
    repoReady,
    busy = false,
    mode = "normal",
    /** Resolved `commit.template` (P10 提交辅助）；per repo, prefills once. */
    template = null,
    oncommit,
    onamend = null,
  }: {
    stagedCount: number;
    repoReady: boolean;
    busy?: boolean;
    /** P8: normal | merge | cherry_pick | revert (operation commit). */
    mode?: "normal" | "merge" | "cherry_pick" | "revert";
    template?: { repoId: string; content: string } | null;
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
  // ---- P10 提交辅助：Co-authored-by 尾注 ----
  let coAuthors = $state<string[]>([]);
  let trailerInput = $state("");
  let trailerOpen = $state(false);

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

  // P10: prefill the commit template when the editor is still untouched.
  // Fires once per repo (switching repos refills; committing does not).
  let templateLoadedFor = $state<string | null>(null);
  $effect(() => {
    if (
      template &&
      template.repoId !== templateLoadedFor &&
      subject.trim() === "" &&
      !amend
    ) {
      loadMessage(template.content);
      templateLoadedFor = template.repoId;
    }
  });

  function reset(): void {
    subject = "";
    description = "";
    amend = false;
    coAuthors = [];
    trailerInput = "";
    trailerOpen = false;
  }

  function addCoAuthor(): void {
    const v = trailerInput.trim();
    if (!v) return;
    trailerInput = "";
    if (!coAuthors.includes(v)) coAuthors = [...coAuthors, v];
  }

  function removeCoAuthor(c: string): void {
    coAuthors = coAuthors.filter((x) => x !== c);
  }

  async function submit(): Promise<void> {
    if (!canCommit) return;
    submitting = true;
    try {
      const desc = description.trim();
      const trailers = coAuthors.map((c) => `Co-authored-by: ${c}`);
      let message = desc ? `${subject.trim()}\n\n${desc}` : subject.trim();
      if (trailers.length > 0) message += `\n\n${trailers.join("\n")}`;
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

  // ---- P10 提交规范辅助：50/72 提示 ----
  const subjectLen = $derived(subject.length);
  const subjectOver = $derived(subjectLen > 50);
  const bodyOver = $derived(
    description.split("\n").some((l) => l.length > 72),
  );
</script>

<div class="space-y-2 border-t bg-background p-2.5">
  {#if mode !== "normal"}
    <div class="text-xs text-amber-600 dark:text-amber-400">
      {t(`conflict.commitHint.${mode}`)}
    </div>
  {/if}
  <div class="relative">
    <Input
      placeholder={t("commit.subject")}
      bind:value={subject}
      disabled={!repoReady}
      class="h-8 text-[13px] {subjectOver ? 'border-amber-500/60 pr-12' : 'pr-12'}"
      {onkeydown}
    />
    {#if subjectLen > 0}
      <span
        class="absolute top-1/2 right-2 -translate-y-1/2 text-[10px] tabular-nums {subjectOver
          ? 'font-medium text-amber-600 dark:text-amber-400'
          : 'text-muted-foreground'}"
        title={t("commit.subjectLenHint")}
      >
        {subjectLen}/50
      </span>
    {/if}
  </div>
  <textarea
    rows={2}
    placeholder={t("commit.description")}
    bind:value={description}
    disabled={!repoReady}
    {onkeydown}
    class="w-full resize-none rounded-md border border-input bg-transparent px-3 py-2 text-[13px] placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-60"
  ></textarea>

  <!-- P10: 50/72 + trailers row -->
  <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
    {#if subjectOver}
      <span class="text-amber-600 dark:text-amber-400">{t("commit.subjectOver50")}</span>
    {/if}
    {#if bodyOver}
      <span class="text-amber-600 dark:text-amber-400">{t("commit.bodyOver72")}</span>
    {/if}
    {#if mode === "normal"}
      <button
        type="button"
        class="flex items-center gap-1 rounded px-1 py-0.5 hover:bg-accent hover:text-foreground"
        onclick={() => (trailerOpen = !trailerOpen)}
        title={t("commit.trailersTitle")}
      >
        <Users class="size-3" />
        {t("commit.trailers")}
      </button>
    {/if}
    <div class="ml-auto flex items-center gap-4">
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
    </div>
  </div>

  {#if trailerOpen}
    <div class="space-y-1.5 rounded-md border bg-muted/30 p-2">
      <div class="flex gap-1.5">
        <Input
          bind:value={trailerInput}
          placeholder={t("commit.trailerPlaceholder")}
          class="h-7 font-mono text-[12px]"
          onkeydown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              addCoAuthor();
            }
          }}
        />
        <Button type="button" variant="outline" size="icon-sm" onclick={addCoAuthor} title={t("commit.trailerAdd")}>
          <Plus class="size-3.5" />
        </Button>
      </div>
      {#if coAuthors.length > 0}
        <div class="flex flex-wrap gap-1">
          {#each coAuthors as c (c)}
            <span class="flex items-center gap-1 rounded bg-muted px-1.5 py-0.5 font-mono text-[11px]">
              {c}
              <button
                type="button"
                class="text-muted-foreground hover:text-foreground"
                onclick={() => removeCoAuthor(c)}
                aria-label={t("commit.trailerRemove")}
              >
                <X class="size-3" />
              </button>
            </span>
          {/each}
        </div>
      {/if}
      <p class="text-[11px] text-muted-foreground">{t("commit.trailerHint")}</p>
    </div>
  {/if}

  <div class="flex items-center">
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
