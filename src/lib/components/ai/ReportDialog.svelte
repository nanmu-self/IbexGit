<script lang="ts">
  /**
   * 日报 / 周报对话框（P11）：范围（今天 / 本周一 00:00 起）、作者过滤
   * （默认 user.email，可切全部成员）、跨仓库手动勾选（当前 + 已打开）。
   * 生成 = 预览 → 流式 Markdown 渲染 → 复制 / 导出 .md / 重新生成。
   * 大范围自动 map-reduce（Rust 侧），预览里的「N 次请求」即批次数。
   */
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { t } from "$lib/i18n";
  import { appDialogs } from "$lib/stores/appdialogs.svelte";
  import { aiStore } from "$lib/stores/ai.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { app, ai, normalizeError, type AiReportRequest } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import { writeText } from "@tauri-apps/plugin-clipboard-manager";
  import { save } from "@tauri-apps/plugin-dialog";
  import { renderMarkdown } from "./markdown";
  import CalendarDays from "@lucide/svelte/icons/calendar-days";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Copy from "@lucide/svelte/icons/copy";
  import Download from "@lucide/svelte/icons/download";
  import Sparkles from "@lucide/svelte/icons/sparkles";

  const open = $derived(appDialogs.aiReportOpen);

  let kind = $state<"daily" | "weekly">("daily");
  let authorMe = $state(true);
  let userEmail = $state<string | null>(null);
  let selected = $state<string[]>([]);
  let dirty = $state(false); // 参数变化后需重新预览

  const phase = $derived(aiStore.reportPhase);
  const streaming = $derived(phase === "streaming");
  const preview = $derived(aiStore.reportPreview);
  const markdown = $derived(renderMarkdown(aiStore.reportText));

  // 打开时：默认参数 + user.email（global/local config 中查找）。
  $effect(() => {
    if (!open) return;
    aiStore.resetReport();
    kind = "daily";
    authorMe = true;
    dirty = true;
    selected = repos.activeId ? [repos.activeId] : [];
    void loadUserEmail();
  });

  async function loadUserEmail(): Promise<void> {
    userEmail = null;
    try {
      const [g, l] = await Promise.all([
        app.configGlobal(),
        repos.activeId ? app.configLocal(repos.activeId) : Promise.resolve([]),
      ]);
      const entry = [...l, ...g].find((e) => e.key === "user.email");
      userEmail = entry?.value ?? null;
    } catch {
      userEmail = null;
    }
  }

  const repoOptions = $derived(
    repos.tabs
      .filter((tab) => tab.id !== "__repos__")
      .map((tab) => ({ id: tab.id, label: tab.name })),
  );

  function toggleRepo(id: string): void {
    dirty = true;
    selected = selected.includes(id)
      ? selected.filter((x) => x !== id)
      : [...selected, id];
  }

  function buildRequest(): AiReportRequest {
    return {
      kind,
      author: authorMe && userEmail ? userEmail : null,
      repo_ids: selected,
      language: settings.locale,
    };
  }

  async function doPreview(): Promise<void> {
    if (selected.length === 0) return;
    dirty = false;
    await aiStore.previewReport(buildRequest());
  }

  async function doGenerate(): Promise<void> {
    if (selected.length === 0) return;
    dirty = false;
    await aiStore.confirmReport(buildRequest());
  }

  function setKind(k: "daily" | "weekly"): void {
    kind = k;
    dirty = true;
  }

  function setAuthor(me: boolean): void {
    authorMe = me;
    dirty = true;
  }

  async function copyReport(): Promise<void> {
    try {
      await writeText(aiStore.reportText);
      showToast("success", t("ai.report.copied"));
    } catch (err) {
      normalizeError(err);
    }
  }

  async function exportReport(): Promise<void> {
    try {
      const path = await save({
        defaultPath: kind === "weekly" ? "weekly-report.md" : "daily-report.md",
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (!path) return;
      await ai.exportMarkdown(path, aiStore.reportText);
      showToast("success", t("ai.report.exported"));
    } catch (err) {
      normalizeError(err);
    }
  }
</script>

<Dialog.Root bind:open={appDialogs.aiReportOpen}>
  <Dialog.Content
    class="flex max-h-[80vh] w-[720px] sm:max-w-[92vw] flex-col gap-3 overflow-hidden"
  >
    <Dialog.Header class="pb-0">
      <Dialog.Title class="flex items-center gap-2">
        <CalendarDays class="size-4" />
        {t("ai.report.title")}
      </Dialog.Title>
      <Dialog.Description>{t("ai.report.desc")}</Dialog.Description>
    </Dialog.Header>

    <!-- 参数区 -->
    <div class="grid grid-cols-3 gap-3 text-[12px]">
      <div class="space-y-1">
        <span class="text-xs text-muted-foreground">{t("ai.report.kind")}</span>
        <div class="grid gap-1">
          <button
            type="button"
            class="rounded-md border px-2 py-1 text-left {kind === 'daily'
              ? 'border-primary bg-primary/10'
              : 'text-muted-foreground hover:bg-accent/50'}"
            onclick={() => setKind("daily")}
          >
            {t("ai.report.daily")}
          </button>
          <button
            type="button"
            class="rounded-md border px-2 py-1 text-left {kind === 'weekly'
              ? 'border-primary bg-primary/10'
              : 'text-muted-foreground hover:bg-accent/50'}"
            onclick={() => setKind("weekly")}
          >
            {t("ai.report.weekly")}
          </button>
        </div>
      </div>

      <div class="space-y-1">
        <span class="text-xs text-muted-foreground">{t("ai.report.author")}</span>
        <div class="grid grid-cols-2 gap-1">
          <button
            type="button"
            class="rounded-md border px-2 py-1 {authorMe
              ? 'border-primary bg-primary/10'
              : 'text-muted-foreground hover:bg-accent/50'}"
            onclick={() => setAuthor(true)}
            title={userEmail ?? t("ai.report.authorUnknown")}
          >
            {t("ai.report.authorMe")}
          </button>
          <button
            type="button"
            class="rounded-md border px-2 py-1 {!authorMe
              ? 'border-primary bg-primary/10'
              : 'text-muted-foreground hover:bg-accent/50'}"
            onclick={() => setAuthor(false)}
          >
            {t("ai.report.authorAll")}
          </button>
        </div>
        {#if authorMe && !userEmail}
          <p class="text-[10px] text-amber-600 dark:text-amber-400">{t("ai.report.authorUnknown")}</p>
        {/if}
      </div>

      <div class="space-y-1">
        <span class="text-xs text-muted-foreground">{t("ai.report.repos")}</span>
        <div class="max-h-20 space-y-1 overflow-y-auto rounded-md border p-1.5">
          {#each repoOptions as r (r.id)}
            <label class="flex items-center gap-1.5 text-[12px]">
              <Checkbox
                checked={selected.includes(r.id)}
                onCheckedChange={() => toggleRepo(r.id)}
              />
              <span class="truncate">{r.label}</span>
            </label>
          {:else}
            <p class="p-1 text-[11px] text-muted-foreground">{t("ai.msg.needStaged")}</p>
          {/each}
        </div>
      </div>
    </div>

    <!-- 预览摘要 -->
    {#if preview}
      <div class="rounded-md border bg-muted/30 px-2.5 py-1.5 text-[11px] text-muted-foreground">
        <div class="flex items-center justify-between gap-2">
          <span>
            {t("ai.report.previewSummary", {
              commits: preview.commits,
              chars: preview.chars,
              batches: preview.batches,
            })}
            {#if preview.excluded > 0}
              · {t("ai.msg.excluded", { n: preview.excluded })}
            {/if}
          </span>
          <span class="truncate">{preview.provider}</span>
        </div>
        <div class="mt-0.5 truncate">
          {#each preview.items as item, i (i)}{#if i > 0} · {/if}{item}{/each}
        </div>
      </div>
    {/if}

    {#if aiStore.reportError}
      <p class="rounded border border-destructive/40 bg-destructive/10 px-2.5 py-1.5 text-[12px] text-destructive">
        {aiStore.reportError}
      </p>
    {/if}

    <!-- 输出区：流式 Markdown 渲染 -->
    <div class="prose-ai min-h-0 flex-1 overflow-y-auto rounded-md border p-3 text-[13px]">
      {#if streaming || aiStore.reportText}
        {@html markdown}
        {#if streaming}
          <p class="mt-2 flex items-center gap-1.5 text-[11px] text-muted-foreground">
            <LoaderCircle class="size-3 animate-spin" />
            {t("ai.report.streamHint")}
            {#if aiStore.reportStatus}
              <span class="truncate">{aiStore.reportStatus}</span>
            {/if}
          </p>
        {/if}
      {:else if phase === "preview" && !preview}
        <p class="flex items-center gap-1.5 text-[12px] text-muted-foreground">
          <LoaderCircle class="size-3 animate-spin" />
          {t("ai.report.preview")}…
        </p>
      {:else}
        <p class="text-[12px] text-muted-foreground">{t("ai.report.empty")}</p>
      {/if}
    </div>

    <!-- 动作区 -->
    <div class="flex items-center gap-2">
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={selected.length === 0 || streaming}
        onclick={() => void doPreview()}
      >
        {t("ai.report.preview")}
      </Button>
      <Button
        type="button"
        size="sm"
        disabled={selected.length === 0 || streaming || (dirty && phase !== "idle")}
        onclick={() => void doGenerate()}
      >
        <Sparkles class="size-3.5" data-icon="inline-start" />
        {phase === "done" || phase === "preview" ? t("ai.report.regenerate") : t("ai.report.confirm")}
      </Button>
      {#if streaming}
        <Button type="button" variant="outline" size="sm" onclick={() => aiStore.abortReport()}>
          {t("ai.report.cancel")}
        </Button>
      {/if}
      <div class="ml-auto flex items-center gap-1.5">
        {#if aiStore.reportText}
          <Button type="button" variant="outline" size="sm" onclick={() => void copyReport()}>
            <Copy class="size-3.5" data-icon="inline-start" />
            {t("ai.report.copy")}
          </Button>
          <Button type="button" variant="outline" size="sm" onclick={() => void exportReport()}>
            <Download class="size-3.5" data-icon="inline-start" />
            {t("ai.report.export")}
          </Button>
        {/if}
      </div>
    </div>
  </Dialog.Content>
</Dialog.Root>
