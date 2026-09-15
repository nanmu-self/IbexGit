<script lang="ts">
  import * as ContextMenu from "$lib/components/ui/context-menu";
  import { t } from "$lib/i18n";
  import { repos, samePath, REPOS_TAB_ID } from "$lib/stores/repos.svelte";
  import { BOOKMARKS, bookmarkColor } from "$lib/bookmarks";
  import X from "@lucide/svelte/icons/x";
  import Plus from "@lucide/svelte/icons/plus";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import LayoutGrid from "@lucide/svelte/icons/layout-grid";
  import Bookmark from "@lucide/svelte/icons/bookmark";

  function closeFromEvent(event: Event, id: string): void {
    event.stopPropagation();
    void repos.close(id);
  }

  function onTabKeydown(event: KeyboardEvent, id: string): void {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      repos.activate(id);
    }
  }

  /**
   * Group separator (P3.5): draw a thin divider between adjacent tabs whose
   * group assignments differ. Tab order itself is never auto-rearranged.
   */
  function sepBefore(i: number): boolean {
    if (i === 0) return false;
    return repos.groupOf(repos.tabs[i - 1].path) !== repos.groupOf(repos.tabs[i].path);
  }
</script>

<div class="relative flex h-9 shrink-0 items-stretch border-b bg-muted/40 text-[13px]">
  <div class="flex min-w-0 flex-1 items-stretch overflow-x-auto">
    {#each repos.tabs as tab, i (tab.id)}
      {#if sepBefore(i)}
        <div class="my-2 w-px shrink-0 bg-border" aria-hidden="true"></div>
      {/if}
      <ContextMenu.Root>
        <ContextMenu.Trigger>
          {#snippet child({ props })}
            {@const bm = bookmarkColor(repos.bookmarkOf(tab.path))}
            <div
              {...props}
              role="tab"
              tabindex="0"
              class="group/tab flex min-w-[110px] max-w-[190px] cursor-pointer items-center gap-1.5 border-r px-2.5 {repos.activeId ===
              tab.id
                ? 'border-b-2 border-b-primary bg-background font-medium'
                : 'border-b-2 border-b-transparent text-muted-foreground hover:bg-background/70'}"
              onclick={() => repos.activate(tab.id)}
              onkeydown={(e) => onTabKeydown(e, tab.id)}
              onauxclick={(e) => {
                if (e.button === 1) void repos.close(tab.id);
              }}
            >
              {#if bm}
                <Bookmark
                  class="size-3 shrink-0 fill-current"
                  style={`color:${bm.color}`}
                  title={t("repos.bookmark")}
                />
              {:else if tab.phase === "loading"}
                <LoaderCircle class="size-3 shrink-0 animate-spin text-muted-foreground" />
              {:else if tab.files.length > 0}
                <span
                  class="size-1.5 shrink-0 rounded-full bg-primary"
                  title={t("sidebar.changes")}
                ></span>
              {:else}
                <span class="size-1.5 shrink-0"></span>
              {/if}
              <span class="min-w-0 flex-1 truncate" title={tab.path}>{tab.name}</span>
              <button
                type="button"
                class="rounded p-0.5 opacity-0 transition-opacity group-hover/tab:opacity-100 hover:bg-muted"
                onclick={(e) => closeFromEvent(e, tab.id)}
                title={t("tabs.close")}
              >
                <X class="size-3" />
              </button>
            </div>
          {/snippet}
        </ContextMenu.Trigger>
        <ContextMenu.Content class="w-48">
          <ContextMenu.Sub>
            <ContextMenu.SubTrigger>{t("repos.moveToGroup")}</ContextMenu.SubTrigger>
            <ContextMenu.SubContent class="w-44">
              {#each repos.groups as g (g.id)}
                <ContextMenu.Item
                  disabled={repos.groupOf(tab.path) === g.id}
                  onSelect={() => void repos.setRepoGroup(tab.path, g.id)}
                >
                  {g.name}
                </ContextMenu.Item>
              {/each}
              {#if repos.groupOf(tab.path)}
                <ContextMenu.Separator />
                <ContextMenu.Item onSelect={() => void repos.setRepoGroup(tab.path, null)}>
                  {t("repos.removeGroup")}
                </ContextMenu.Item>
              {/if}
            </ContextMenu.SubContent>
          </ContextMenu.Sub>
          <ContextMenu.Sub>
            <ContextMenu.SubTrigger>{t("repos.bookmark")}</ContextMenu.SubTrigger>
            <ContextMenu.SubContent class="w-40">
              <div class="grid grid-cols-4 gap-1 p-1.5">
                {#each BOOKMARKS as b (b.id)}
                  <button
                    type="button"
                    class="flex size-7 items-center justify-center rounded hover:bg-accent {repos.bookmarkOf(tab.path) ===
                    b.id
                      ? 'bg-accent'
                      : ''}"
                    title={b.id}
                    onclick={() => void repos.setBookmark(tab.path, b.id)}
                  >
                    <Bookmark class="size-4 fill-current" style={`color:${b.color}`} />
                  </button>
                {/each}
              </div>
              {#if repos.bookmarkOf(tab.path)}
                <ContextMenu.Separator />
                <ContextMenu.Item onSelect={() => void repos.setBookmark(tab.path, null)}>
                  {t("repos.removeBookmark")}
                </ContextMenu.Item>
              {/if}
            </ContextMenu.SubContent>
          </ContextMenu.Sub>
          <ContextMenu.Separator />
          <ContextMenu.Item onSelect={() => void repos.close(tab.id)}>
            {t("tabs.close")}
          </ContextMenu.Item>
          <ContextMenu.Item onSelect={() => void repos.close(tab.id, true)}>
            {t("tabs.closeOthers")}
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Root>
    {/each}
  </div>

  <!-- Repositories overview tab (P3.5): pinned next to "+", always reachable. -->
  {#if repos.reposTabOpen}
    <ContextMenu.Root>
      <ContextMenu.Trigger>
        {#snippet child({ props })}
          <div
            {...props}
            role="tab"
            tabindex="0"
            class="group/repos flex shrink-0 cursor-pointer items-center gap-1.5 border-l px-2.5 {repos.activeId ===
            REPOS_TAB_ID
              ? 'border-b-2 border-b-primary bg-background font-medium'
              : 'border-b-2 border-b-transparent text-muted-foreground hover:bg-background/70'}"
            onclick={() => repos.activate(REPOS_TAB_ID)}
            onkeydown={(e) => onTabKeydown(e, REPOS_TAB_ID)}
            onauxclick={(e) => {
              if (e.button === 1) void repos.close(REPOS_TAB_ID);
            }}
          >
            <LayoutGrid class="size-3.5 shrink-0" />
            <span class="whitespace-nowrap">{t("tabs.reposTab")}</span>
            <button
              type="button"
              class="rounded p-0.5 opacity-0 transition-opacity group-hover/repos:opacity-100 hover:bg-muted"
              onclick={(e) => closeFromEvent(e, REPOS_TAB_ID)}
              title={t("tabs.close")}
            >
              <X class="size-3" />
            </button>
          </div>
        {/snippet}
      </ContextMenu.Trigger>
      <ContextMenu.Content class="w-44">
        <ContextMenu.Item onSelect={() => void repos.close(REPOS_TAB_ID)}>
          {t("tabs.close")}
        </ContextMenu.Item>
      </ContextMenu.Content>
    </ContextMenu.Root>
  {/if}

  <!-- "+": open the repositories overview tab (P3.5). -->
  <div class="flex items-stretch border-l border-border/40">
    <button
      type="button"
      class="flex w-9 items-center justify-center text-muted-foreground hover:bg-background/70 hover:text-foreground"
      onclick={() => repos.openReposTab()}
      title={t("tabs.addTab")}
    >
      <Plus class="size-4" />
    </button>
  </div>
</div>
