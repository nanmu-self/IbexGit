<script lang="ts">
  /**
   * OperationBanner (P8 仓库状态头): guidance bar shown while a
   * merge / rebase / cherry-pick / revert sequence is in progress.
   * Abort asks for confirmation; continue/skip surface git errors as toasts.
   */
  import { Button } from "$lib/components/ui/button";
  import { ConfirmDialog } from "$lib/components/ui/confirm-dialog";
  import { t } from "$lib/i18n";
  import { git, normalizeError, type OperationState } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import GitMerge from "@lucide/svelte/icons/git-merge";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import GitCherryPick from "@lucide/svelte/icons/cherry";
  import GitPullRequestArrow from "@lucide/svelte/icons/git-pull-request-arrow";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Undo2 from "@lucide/svelte/icons/undo-2";
  import Play from "@lucide/svelte/icons/play";
  import SkipForward from "@lucide/svelte/icons/skip-forward";
  import ListTodo from "@lucide/svelte/icons/list-todo";

  let {
    repoId,
    operation,
    conflictCount,
    onchanged,
    onresolve,
  }: {
    repoId: string;
    operation: OperationState;
    conflictCount: number;
    onchanged: () => void;
    onresolve: () => void;
  } = $props();

  let busy = $state(false);
  let abortOpen = $state(false);

  const short = (sha: string | null): string => (sha ? sha.slice(0, 7) : "");

  const summary = $derived.by(() => {
    const op = operation;
    const onto = short(op.onto);
    switch (op.kind) {
      case "merge":
        return {
          icon: GitMerge,
          text: op.message
            ? t("conflict.op.mergeMsg", { subject: op.message.split("\n")[0] })
            : t("conflict.op.merge", { onto }),
          skip: false,
        };
      case "rebase":
        return {
          icon: GitBranch,
          text: t("conflict.op.rebase", { onto, step: op.step ?? "?", total: op.total ?? "?" }),
          skip: true,
        };
      case "cherry_pick":
        return {
          icon: GitCherryPick,
          text: t("conflict.op.cherryPick", { onto }),
          skip: true,
        };
      case "revert":
        return {
          icon: Undo2,
          text: t("conflict.op.revert", { onto }),
          skip: true,
        };
      case "apply":
        return { icon: GitPullRequestArrow, text: t("conflict.op.apply"), skip: true };
      case "bisect":
        return { icon: ListTodo, text: t("conflict.op.bisect"), skip: false };
    }
  });

  async function run(action: "continue" | "skip" | "abort"): Promise<void> {
    if (busy) return;
    busy = true;
    try {
      if (action === "continue") await git.operationContinue(repoId);
      else if (action === "skip") await git.operationSkip(repoId);
      else await git.operationAbort(repoId);
      if (action === "continue") showToast("success", t("conflict.op.continueDone"));
      else if (action === "skip") showToast("success", t("conflict.op.skipDone"));
      else showToast("info", t("conflict.op.abortDone"));
      onchanged();
    } catch (e) {
      normalizeError(e);
    } finally {
      busy = false;
    }
  }
</script>

<div
  class="flex flex-wrap items-center gap-2 border-b border-warning/30 bg-warning-surface px-3 py-1.5 text-sm"
>
  <summary.icon class="size-4 shrink-0 text-warning dark:text-warning" />
  <span class="min-w-0 flex-1 truncate text-warning dark:text-warning">
    {summary.text}
  </span>

  {#if conflictCount > 0}
    <Button
      size="sm"
      class="h-7 bg-danger text-xs text-danger-foreground hover:bg-danger/90"
      onclick={onresolve}
    >
      <CircleAlert class="size-3.5" />
      {t("conflict.op.resolveN", { n: conflictCount })}
    </Button>
  {:else}
    <span class="text-xs text-warning dark:text-warning/80">
      {t("conflict.op.noConflicts")}
    </span>
  {/if}

  <div class="flex items-center gap-1">
    {#if summary.skip && conflictCount === 0}
      <Button variant="ghost" size="xs" class="text-xs" disabled={busy} onclick={() => run("skip")}>
        <SkipForward class="size-3.5" />
        {t("conflict.op.skip")}
      </Button>
    {/if}
    {#if operation.kind !== "bisect"}
      <Button variant="ghost" size="xs" class="text-xs" disabled={busy} onclick={() => run("continue")}>
        {#if busy}
          <LoaderCircle class="size-3.5 animate-spin" />
        {:else}
          <Play class="size-3.5" />
        {/if}
        {t("conflict.op.continue")}
      </Button>
    {/if}
    <Button
      variant="ghost"
      size="xs"
      class="text-xs text-danger hover:text-danger dark:text-danger"
      disabled={busy}
      onclick={() => (abortOpen = true)}
    >
      <Undo2 class="size-3.5" />
      {t("conflict.op.abort")}
    </Button>
  </div>
</div>

<ConfirmDialog
  bind:open={abortOpen}
  title={t("conflict.op.abortConfirmTitle")}
  description={t("conflict.op.abortConfirmDesc")}
  confirmLabel={t("conflict.op.abort")}
  destructive
  onconfirm={() => run("abort")}
/>
