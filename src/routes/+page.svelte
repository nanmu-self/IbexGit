<script lang="ts">
  import TitleBar from "$lib/components/layout/TitleBar.svelte";
  import Toolbar from "$lib/components/layout/Toolbar.svelte";
  import RepoTabs from "$lib/components/layout/RepoTabs.svelte";
  import ReposTab from "$lib/components/layout/ReposTab.svelte";
  import Sidebar from "$lib/components/layout/Sidebar.svelte";
  import StatusBar from "$lib/components/layout/StatusBar.svelte";
  import Welcome from "$lib/components/welcome/Welcome.svelte";
  import WorkspaceView from "$lib/components/workspace/WorkspaceView.svelte";
  import HistoryView from "$lib/components/history/HistoryView.svelte";
  import TagsView from "$lib/components/refs/TagsView.svelte";
  import FileInspectDialog from "$lib/components/file/FileInspectDialog.svelte";
  import RefsDialogsHost from "$lib/components/refs/RefsDialogsHost.svelte";
  import NetDialogsHost from "$lib/components/credential/NetDialogsHost.svelte";
  import GitErrorDialog from "$lib/components/giterror/GitErrorDialog.svelte";
  import ReportDialog from "$lib/components/ai/ReportDialog.svelte";
  import { wireAiEvents } from "$lib/stores/ai.svelte";
  import { EmptyState } from "$lib/components/ui/empty-state";
  import { PanelResizer } from "$lib/components/ui/panel-resizer";
  import { Button } from "$lib/components/ui/button";
  import { settings } from "$lib/stores/settings.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import { onAction } from "$lib/keyboard";
  import { initFeatureShortcuts } from "$lib/features";
  import { t } from "$lib/i18n";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";

  let restored = $state(false);
  /** PanelResizer drag state: Sidebar drops its width transition while true. */
  let sidebarResizing = $state(false);

  // P11: AI 生成事件订阅（幂等，全局一份）。
  wireAiEvents();

  // Session restore (P2 acceptance): reopen last repos once settings are in.
  $effect(() => {
    if (settings.ready && !restored) {
      restored = true;
      void repos.restoreSession();
    }
  });

  function stepTab(delta: number): void {
    const list = repos.tabs;
    if (list.length === 0) return;
    const idx = list.findIndex((t) => t.id === repos.activeId);
    const next = list[(idx + delta + list.length) % list.length];
    repos.activate(next.id);
  }

  $effect(() => {
    const offs = [
      // P10：所有 feature 声明的快捷键由注册表统一接线（含打开/刷新/侧栏/
      // 任务中心等），这里只保留非 feature 的 Tab 切换动作。
      initFeatureShortcuts(),
      onAction("repo.nextTab", () => stepTab(1)),
      onAction("repo.prevTab", () => stepTab(-1)),
    ];
    return () => offs.forEach((off) => off());
  });

  const active = $derived(repos.active);
</script>

{#if !settings.ready}
  <div class="flex h-screen items-center justify-center gap-2 text-sm text-muted-foreground">
    <LoaderCircle class="size-4 animate-spin" />
    {t("app.loading")}
  </div>
{:else if repos.tabs.length === 0}
  <div class="flex h-screen flex-col overflow-hidden">
    <TitleBar />
    <Welcome />
    <StatusBar />
  </div>
{:else}
  <div class="flex h-screen flex-col overflow-hidden">
    <TitleBar />
    <RepoTabs />
    <Toolbar />
    <div class="flex min-h-0 flex-1">
      {#if settings.showSidebar}
        <Sidebar resizing={sidebarResizing} />
        <PanelResizer
          bind:width={settings.sidebarWidth}
          bind:dragging={sidebarResizing}
          min={200}
          max={420}
          onCommit={(w) => void settings.setSidebarWidth(w)}
        />
      {/if}
      <main class="flex min-w-0 flex-1 flex-col">
        {#if repos.reposTabActive}
          <ReposTab />
        {:else if !active || active.phase === "loading"}
          <div class="flex flex-1 items-center justify-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle class="size-4 animate-spin" />
            {t("common.loading")}
          </div>
        {:else if active.phase === "error"}
          <EmptyState
            icon={CircleAlert}
            title={t("common.openRepoFailed")}
            hint={active.error ?? ""}
          >
            {#snippet action()}
              <Button variant="outline" size="sm" onclick={() => void repos.refresh(active.id)}>
                {t("statusbar.refresh")}
              </Button>
            {/snippet}
          </EmptyState>
        {:else if repos.ui.view === "history"}
          <HistoryView />
        {:else if repos.ui.view === "tags"}
          <TagsView />
        {:else}
          <WorkspaceView />
        {/if}
      </main>
    </div>
    <StatusBar />
    <RefsDialogsHost />
    <GitErrorDialog />
    <FileInspectDialog />
    <ReportDialog />
  </div>
{/if}

<!-- 全局网络对话框（克隆 / 新建仓库 / 凭据）：Portal 渲染，
     必须始终挂载 —— 欢迎页（无仓库 Tab）也要能打开克隆/新建。 -->
<NetDialogsHost />
