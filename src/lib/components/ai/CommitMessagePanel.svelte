<script lang="ts">
  /**
   * AI 提交消息面板（P11，PLAN P11「生成 UI」）：嵌入 CommitBox。
   * 流程 = 发送前预览（隐私红线）→ 确认生成（Task 化、可取消、流式）→
   * done（自动插入提交框，可继续编辑；候选列表支持多方案重新生成）。
   */
  import { Button } from "$lib/components/ui/button";
  import { t } from "$lib/i18n";
  import { aiStore } from "$lib/stores/ai.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { showToast } from "$lib/stores/toast";
  import Sparkles from "@lucide/svelte/icons/sparkles";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import ClipboardCheck from "@lucide/svelte/icons/clipboard-check";
  import X from "@lucide/svelte/icons/x";

  interface Props {
    stagedCount: number;
    /** 把生成的完整消息插入提交框（subject + body 可继续编辑）。 */
    oninsert: (fullMessage: string) => void;
  }
  let { stagedCount, oninsert }: Props = $props();

  const repoId = $derived(repos.activeId);
  const phase = $derived(aiStore.msgPhase);
  const streaming = $derived(phase === "streaming");

  function open(): void {
    if (!repoId) return;
    if (stagedCount === 0) {
      showToast("info", t("ai.msg.needStaged"));
      return;
    }
    aiStore.openMessagePanel(settings.locale);
    void aiStore.previewMessage(repoId);
  }

  function confirm(): void {
    if (repoId) void aiStore.confirmMessage(repoId);
  }

  function regenerate(): void {
    if (repoId) void aiStore.regenerateMessage(repoId);
  }

  /** 生成完成即自动插入（插入后提交框内容可继续编辑）。 */
  $effect(() => {
    if (phase === "done") {
      const latest = aiStore.takeLatestCandidate();
      if (latest) oninsert(latest);
    }
  });

  function insertCandidate(text: string): void {
    oninsert(text);
  }
</script>

<div class="relative flex flex-col items-end">
  <button
    type="button"
    class="flex items-center gap-1 rounded px-1 py-0.5 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground {aiStore
      .msgOpen
      ? 'bg-accent text-foreground'
      : ''}"
    title={t("ai.msg.button")}
    onclick={open}
  >
    <Sparkles class="size-3" />
    AI
  </button>

  {#if aiStore.msgOpen}
    <div
      class="absolute right-0 bottom-full z-30 mb-1 max-h-[min(420px,60vh)] w-[380px] space-y-2 overflow-y-auto rounded-md border bg-popover p-2.5 text-[12px] shadow-md"
      role="dialog"
      aria-label={t("ai.msg.title")}
    >
      <div class="flex items-center justify-between">
        <span class="flex items-center gap-1 text-xs font-semibold">
          <Sparkles class="size-3" />
          {t("ai.msg.title")}
        </span>
        <button
          type="button"
          class="text-muted-foreground hover:text-foreground"
          onclick={() => aiStore.closeMessagePanel()}
          aria-label={t("ai.report.cancel")}
        >
          <X class="size-3.5" />
        </button>
      </div>

      {#if aiStore.msgError}
        <p class="rounded border border-destructive/40 bg-destructive/10 px-2 py-1.5 text-[11px] text-destructive">
          {aiStore.msgError}
        </p>
      {/if}

      {#if phase === "preview" && aiStore.msgPreview}
        <div class="space-y-1.5">
          <p class="text-[11px] font-medium">{t("ai.msg.previewTitle")}</p>
          <p class="text-[11px] text-muted-foreground">
            {t("ai.msg.previewBody", {
              files: aiStore.msgPreview.items.length,
              chars: aiStore.msgPreview.chars,
              provider: aiStore.msgPreview.provider,
            })}
          </p>
          {#if aiStore.msgPreview.excluded > 0}
            <p class="text-[11px] text-green-600 dark:text-green-400">
              {t("ai.msg.excluded", { n: aiStore.msgPreview.excluded })}
            </p>
          {/if}
          {#if aiStore.msgPreview.truncated}
            <p class="text-[11px] text-amber-600 dark:text-amber-400">{t("ai.msg.truncated")}</p>
          {/if}
          <div class="max-h-24 overflow-y-auto rounded border bg-muted/30 p-1.5 font-mono text-[10px] text-muted-foreground">
            {#each aiStore.msgPreview.items as item (item)}
              <div class="truncate">{item}</div>
            {/each}
          </div>
          <div class="flex justify-end gap-1.5 pt-0.5">
            <Button type="button" variant="outline" size="sm" onclick={() => aiStore.closeMessagePanel()}>
              {t("ai.report.cancel")}
            </Button>
            <Button type="button" size="sm" onclick={confirm}>
              <Sparkles class="size-3" data-icon="inline-start" />
              {t("ai.msg.confirm")}
            </Button>
          </div>
        </div>
      {:else if streaming}
        <div class="space-y-1.5">
          <p class="flex items-center gap-1.5 text-[11px] text-muted-foreground">
            <LoaderCircle class="size-3 animate-spin" />
            {t("ai.msg.streamHint")}
            {#if aiStore.msgStatus}
              <span class="truncate">{aiStore.msgStatus}</span>
            {/if}
          </p>
          <pre
            class="max-h-32 overflow-y-auto rounded border bg-muted/30 p-1.5 font-mono text-[11px] whitespace-pre-wrap">{aiStore.msgText}</pre>
          <div class="flex justify-end">
            <Button type="button" variant="outline" size="sm" onclick={() => aiStore.abortMessage()}>
              {t("ai.report.cancel")}
            </Button>
          </div>
        </div>
      {:else if phase === "done" && aiStore.msgCandidates.length > 0}
        <div class="space-y-1.5">
          {#each aiStore.msgCandidates as c, i (i)}
            <div class="rounded border bg-muted/30 p-1.5">
              <div class="flex items-start justify-between gap-2">
                <pre
                  class="min-w-0 flex-1 font-mono text-[11px] whitespace-pre-wrap">{c}</pre>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  title={t("ai.msg.insert")}
                  onclick={() => insertCandidate(c)}
                >
                  <ClipboardCheck class="size-3.5" />
                </Button>
              </div>
            </div>
          {/each}
          <div class="flex justify-end">
            <Button type="button" variant="outline" size="sm" disabled={streaming} onclick={regenerate}>
              <Sparkles class="size-3" data-icon="inline-start" />
              {t("ai.msg.regenerate")}
            </Button>
          </div>
        </div>
      {/if}
    </div>
  {/if}
</div>
