<script lang="ts">
  /**
   * TagsView (P6) — full tag management in the content area: annotated /
   * lightweight tags with target, tagger, date and message; create
   * (dialog), checkout (detached) and delete via the refs action bus.
   */
  import { Button } from "$lib/components/ui/button";
  import { EmptyState } from "$lib/components/ui/empty-state";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { refsData, loadRefsData } from "$lib/stores/refsdata.svelte";
  import { requestRefAction } from "$lib/stores/refbus";
  import { git, normalizeError } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import TagIcon from "@lucide/svelte/icons/tag";
  import Plus from "@lucide/svelte/icons/plus";
  import CircleDot from "@lucide/svelte/icons/circle-dot";
  import Scissors from "@lucide/svelte/icons/scissors";

  const active = $derived(repos.active);
  const tags = $derived(refsData.tags);

  // Keep the shared refs data fresh (repo switch + watcher refresh).
  let lastRefreshMark: number | null = null;
  $effect(() => {
    const id = repos.activeId;
    const mark = active?.lastRefreshMs ?? null;
    if (id === null) return;
    if (mark !== lastRefreshMark) {
      lastRefreshMark = mark;
      void loadRefsData(id);
    }
  });

  async function checkoutTag(name: string): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    try {
      await git.checkoutBranch(id, name);
      showToast("success", t("sidebar.checkoutDone", { name }));
      await repos.refresh(id);
      await loadRefsData(id);
    } catch (e) {
      normalizeError(e);
    }
  }

  async function deleteTag(name: string): Promise<void> {
    const id = repos.activeId;
    if (id === null) return;
    try {
      await git.deleteTag(id, name);
      showToast("success", t("refs.tagDeleted", { name }));
      await repos.refresh(id);
      await loadRefsData(id);
    } catch (e) {
      normalizeError(e);
    }
  }
</script>

<div class="flex min-h-0 flex-1 flex-col">
  <div class="flex items-center gap-2 border-b px-3 py-2">
    <h2 class="text-[13px] font-medium">{t("sidebar.tagsView")}</h2>
    <span class="text-xs text-muted-foreground">{tags.length}</span>
    <div class="ml-auto flex items-center gap-1">
      <Button
        variant="ghost"
        size="sm"
        disabled={!active}
        onclick={() => requestRefAction({ kind: "newTag" })}
      >
        <Plus class="size-3.5" /> {t("refs.tagDialog.create")}
      </Button>
    </div>
  </div>

  {#if tags.length === 0}
    <EmptyState
      icon={TagIcon}
      title={t("refs.tags.none")}
      hint={t("refs.tags.emptyHint")}
    >
      {#snippet action()}
        <Button
          variant="outline"
          size="sm"
          disabled={!active}
          onclick={() => requestRefAction({ kind: "newTag" })}
        >
          <Plus class="size-3.5" /> {t("refs.tagDialog.title")}
        </Button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="min-h-0 flex-1 overflow-y-auto p-3">
      <div class="mx-auto max-w-2xl divide-y rounded-lg border">
        {#each tags as tag (tag.name)}
          <div class="flex items-center gap-3 px-3 py-2">
            <TagIcon class="size-4 shrink-0 text-muted-foreground" />
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-2">
                <span class="truncate text-[13px] font-medium">{tag.name}</span>
                {#if tag.message}
                  <span class="truncate text-xs text-muted-foreground">{tag.message}</span>
                {:else}
                  <span class="rounded bg-muted px-1 text-[10px] text-muted-foreground">
                    {t("refs.tags.lightweight")}
                  </span>
                {/if}
              </div>
              <div class="truncate text-[11px] text-muted-foreground">
                {tag.target.slice(0, 7)} · {tag.tagger ?? ""} · {tag.date ?? ""}
              </div>
            </div>
            <Button
              variant="ghost"
              size="xs"
              title={t("refs.menu.checkoutDetached")}
              onclick={() => checkoutTag(tag.name)}
            >
              <CircleDot class="size-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="xs"
              title={t("common.delete")}
              onclick={() => deleteTag(tag.name)}
            >
              <Scissors class="size-3.5" />
            </Button>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>
