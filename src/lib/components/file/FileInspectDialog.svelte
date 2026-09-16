<script lang="ts">
  // P9 文件追溯窗口：单文件历史（log --follow + diff 预览）与 Blame 两个
  // 页签；由 fileView store 驱动（工作区右键 / 提交详情文件行右键 /
  // 冲突列表入口共用）。Blame 点击行 → 跳到 history 页该提交的 diff。
  import * as Dialog from "$lib/components/ui/dialog";
  import FileHistoryList from "./FileHistoryList.svelte";
  import BlameView from "./BlameView.svelte";
  import { fileView, type FileViewTab } from "$lib/stores/fileview.svelte";
  import { t } from "$lib/i18n";
  import History from "@lucide/svelte/icons/history";
  import TextSelect from "@lucide/svelte/icons/text-select";

  const TABS: { id: FileViewTab; label: () => string; icon: typeof History }[] = [
    { id: "history", label: () => t("fileView.historyTab"), icon: History },
    { id: "blame", label: () => t("fileView.blameTab"), icon: TextSelect },
  ];

  function switchTab(tab: FileViewTab): void {
    if (fileView.path) fileView.show(fileView.path, tab);
  }
</script>

<Dialog.Root bind:open={fileView.open}>
  <Dialog.Content class="flex h-[88vh] max-w-6xl flex-col gap-0 overflow-hidden p-0">
    <div class="flex items-center gap-2 border-b py-2.5 pr-12 pl-4">
      <div class="flex min-w-0 flex-1 flex-col">
        <Dialog.Title class="truncate font-mono text-sm font-medium" title={fileView.path ?? ""}>
          {fileView.path}
        </Dialog.Title>
        <Dialog.Description class="text-xs text-muted-foreground">
          {fileView.tab === "history" ? t("fileView.historyHint") : t("fileView.blameHint")}
        </Dialog.Description>
      </div>
      <div class="flex shrink-0 items-center rounded-md bg-muted/60 p-0.5">
        {#each TABS as tab (tab.id)}
          <button
            class="flex items-center gap-1 rounded px-2.5 py-1 text-xs {fileView.tab === tab.id
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground'}"
            onclick={() => switchTab(tab.id)}
          >
            <tab.icon class="size-3.5" />
            {tab.label()}
          </button>
        {/each}
      </div>
    </div>

    <div class="flex min-h-0 flex-1 flex-col">
      {#if fileView.tab === "history" && fileView.path}
        <FileHistoryList path={fileView.path} jumpHash={fileView.jumpHash} />
      {:else if fileView.path}
        <BlameView path={fileView.path} onjump={(hash) => fileView.jumpToCommit(hash)} />
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>
