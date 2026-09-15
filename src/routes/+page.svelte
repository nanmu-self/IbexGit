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
  import RefsDialogsHost from "$lib/components/refs/RefsDialogsHost.svelte";
  import { EmptyState } from "$lib/components/ui/empty-state";
  import { PanelResizer } from "$lib/components/ui/panel-resizer";
  import { Button } from "$lib/components/ui/button";
  import { settings } from "$lib/stores/settings.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import { onAction } from "$lib/keyboard";
  import { pickRepo } from "$lib/repo-picker";
  import { t } from "$lib/i18n";
  import { showToast } from "$lib/stores/toast";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import CircleAlert from "@lucide/svelte/icons/circle-alert";

  let restored = $state(false);
  /** PanelResizer drag state: Sidebar drops its width transition while true. */
  let sidebarResizing = $state(false);

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
      onAction("repo.open", () => void pickRepo()),
      onAction("repo.refresh", () => {
        const id = repos.activeId;
        if (id !== null) void repos.refresh(id);
      }),
      onAction("view.toggleSidebar", () => void settings.setShowSidebar(!settings.showSidebar)),
      onAction("view.toggleTheme", () => {
        const order = ["light", "dark", "system"] as const;
        const next = order[(order.indexOf(settings.theme) + 1) % order.length];
        void settings.setTheme(next);
      }),
      onAction("repo.nextTab", () => stepTab(1)),
      onAction("repo.prevTab", () => stepTab(-1)),
      onAction("app.tasks", () => showToast("info", t("common.comingSoon", { phase: "P3" }))),
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
  </div>
{/if}
