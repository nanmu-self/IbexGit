<script lang="ts">
  /**
   * Repositories overview tab (P3.5): opened by the "+" button like a
   * browser's new-tab page. Lists every known repository (recents ∪
   * groups.json entries) under Bookmarks → Groups → Ungrouped, with full
   * group CRUD (create / rename / delete / assign) and colorful bookmarks.
   * Clicking an entry opens or activates that repo's tab.
   */
  import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
  import * as Dialog from "$lib/components/ui/dialog";
  import { ConfirmDialog } from "$lib/components/ui/confirm-dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { t } from "$lib/i18n";
  import { repos, samePath } from "$lib/stores/repos.svelte";
  import { pickRepo } from "$lib/repo-picker";
  import { showToast } from "$lib/stores/toast";
  import { netDialogs } from "$lib/stores/netdialogs.svelte";
  import { appDialogs } from "$lib/stores/appdialogs.svelte";
  import { BOOKMARKS, bookmarkColor } from "$lib/bookmarks";
  import type { RepoGroup } from "$lib/git/bindings";
  import Search from "@lucide/svelte/icons/search";
  import Bookmark from "@lucide/svelte/icons/bookmark";
  import Folder from "@lucide/svelte/icons/folder";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import CloudDownload from "@lucide/svelte/icons/cloud-download";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Pencil from "@lucide/svelte/icons/pencil";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";

  let query = $state("");

  // Group name prompt (create / rename).
  let promptOpen = $state(false);
  let promptTitle = $state("");
  let promptValue = $state("");
  let promptEditingId = $state<string | null>(null);
  /** When creating from an entry's group picker, assign that repo after. */
  let promptAssignPath = $state<string | null>(null);

  // Group delete confirmation.
  let deleteOpen = $state(false);
  let deleteTarget = $state<RepoGroup | null>(null);

  interface RepoEntry {
    path: string;
    name: string;
    lastOpened: number;
    bookmark: string | null;
    groupId: string | null;
    isOpen: boolean;
  }

  /** Known repos = recents ∪ meaningful groups.json entries. */
  const entries = $derived.by(() => {
    const list: RepoEntry[] = [];
    const find = (path: string) => list.find((e) => samePath(e.path, path));
    const add = (path: string, name: string | undefined, lastOpened: number) => {
      const existing = find(path);
      if (existing) {
        existing.lastOpened = Math.max(existing.lastOpened, lastOpened);
        if (name && lastOpened >= existing.lastOpened) existing.name = name;
        return;
      }
      list.push({
        path,
        name: name ?? path.split(/[\\/]/).filter(Boolean).pop() ?? path,
        lastOpened,
        bookmark: repos.bookmarkOf(path),
        groupId: repos.groupOf(path),
        isOpen: repos.tabs.some((tab) => samePath(tab.path, path)),
      });
    };
    for (const r of repos.recent) add(r.path, r.name, r.last_opened ?? 0);
    for (const [path, meta] of Object.entries(repos.repoMeta)) {
      if (meta.group_id || meta.bookmark) add(path, undefined, 0);
    }
    return list;
  });

  const byRecency = (a: RepoEntry, b: RepoEntry) =>
    b.lastOpened - a.lastOpened || a.name.localeCompare(b.name);

  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter(
      (e) => e.name.toLowerCase().includes(q) || e.path.toLowerCase().includes(q),
    );
  });

  const bookmarkOrder = (id: string | null): number =>
    id ? BOOKMARKS.findIndex((b) => b.id === id) : BOOKMARKS.length;

  const sections = $derived.by(() => {
    const bookmarks = filtered
      .filter((e) => e.bookmark)
      .sort((a, b) => bookmarkOrder(a.bookmark) - bookmarkOrder(b.bookmark) || byRecency(a, b));
    const groups = repos.groups
      .map((g) => ({
        group: g,
        items: filtered.filter((e) => e.groupId === g.id).sort(byRecency),
      }))
      .filter((s) => s.items.length > 0);
    const ungrouped = filtered.filter((e) => !e.groupId).sort(byRecency);
    return { bookmarks, groups, ungrouped };
  });

  function openEntry(entry: RepoEntry): void {
    void repos.openPath(entry.path);
  }

  function openFolder(): void {
    void pickRepo();
  }

  function startCreateGroup(assignPath: string | null = null): void {
    promptEditingId = null;
    promptAssignPath = assignPath;
    promptValue = "";
    promptTitle = t("repos.newGroupTitle");
    promptOpen = true;
  }

  function startRenameGroup(group: RepoGroup): void {
    promptEditingId = group.id;
    promptAssignPath = null;
    promptValue = group.name;
    promptTitle = t("repos.renameGroupTitle");
    promptOpen = true;
  }

  async function submitPrompt(): Promise<void> {
    const name = promptValue.trim();
    if (!name) return;
    try {
      if (promptEditingId !== null) {
        await repos.renameGroup(promptEditingId, name);
      } else {
        const created = await repos.createGroup(name);
        if (promptAssignPath) {
          await repos.setRepoGroup(promptAssignPath, created.id);
        }
      }
      promptOpen = false;
    } catch {
      /* toast already surfaced by the error pipeline */
    }
  }

  function askDeleteGroup(group: RepoGroup): void {
    deleteTarget = group;
    deleteOpen = true;
  }

  async function confirmDeleteGroup(): Promise<void> {
    if (!deleteTarget) return;
    try {
      await repos.deleteGroup(deleteTarget.id);
    } catch {
      /* toast already surfaced */
    } finally {
      deleteTarget = null;
    }
  }

  function onSearchKeydown(event: KeyboardEvent): void {
    if (event.key === "Enter") {
      event.preventDefault();
      const first = sections.bookmarks[0] ??
        sections.groups[0]?.items[0] ?? sections.ungrouped[0];
      if (first) openEntry(first);
    }
  }
</script>

<div class="min-h-0 flex-1 overflow-y-auto">
  <div class="mx-auto w-full max-w-3xl space-y-6 p-8">
    <!-- Header -->
    <div class="flex items-center justify-between gap-4">
      <div>
        <h1 class="text-lg font-semibold">{t("repos.title")}</h1>
        <p class="text-xs text-muted-foreground">{t("repos.count", { n: entries.length })}</p>
      </div>
      <div class="flex items-center gap-2">
        <Button variant="outline" size="sm" onclick={() => startCreateGroup()}>
          <FolderPlus class="size-4" />
          {t("repos.newGroup")}
        </Button>
        <Button variant="outline" size="sm" onclick={openFolder}>
          <FolderOpen class="size-4" />
          {t("repos.openFolder")}
        </Button>
        <Button variant="outline" size="sm" onclick={() => netDialogs.openClone()}>
          <CloudDownload class="size-4" />
          {t("repos.clone")}
        </Button>
        <Button variant="outline" size="sm" onclick={() => netDialogs.openNewRepo()}>
          <FolderPlus class="size-4" />
          {t("repos.newRepo")}
        </Button>
        <Button variant="outline" size="sm" onclick={() => appDialogs.openSettings("credentials")}>
          <KeyRound class="size-4" />
          {t("repos.credentials")}
        </Button>
      </div>
    </div>

    <!-- Search -->
    <div class="flex h-10 items-center gap-2 rounded-lg border bg-background px-3 shadow-xs">
      <Search class="size-4 shrink-0 text-muted-foreground" />
      <input
        bind:value={query}
        onkeydown={onSearchKeydown}
        aria-label={t("repos.search")}
        placeholder={t("repos.search")}
        spellcheck="false"
        class="h-full w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
      />
    </div>

    <!-- Lists -->
    {#if filtered.length === 0}
      <div class="rounded-lg border border-dashed p-10 text-center text-sm text-muted-foreground">
        {#if entries.length === 0}
          {t("repos.empty")}
        {:else}
          {t("repos.noResults")}
        {/if}
      </div>
    {:else}
      {#if sections.bookmarks.length > 0}
        <section>
          <h2 class="mb-2 flex items-center gap-1.5 text-xs font-medium tracking-wide text-muted-foreground uppercase">
            <Bookmark class="size-3.5" />
            {t("repos.bookmarks")}
          </h2>
          <div class="overflow-hidden rounded-lg border divide-y">
            {#each sections.bookmarks as entry (entry.path)}
              {@render entryRow(entry)}
            {/each}
          </div>
        </section>
      {/if}

      {#each sections.groups as section (section.group.id)}
        <section>
          <div class="group/header mb-2 flex items-center gap-1.5">
            <h2 class="text-xs font-medium tracking-wide text-muted-foreground uppercase">
              {section.group.name}
            </h2>
            <span class="text-[11px] text-muted-foreground/70">{section.items.length}</span>
            <div class="ml-1 flex items-center gap-0.5 opacity-0 transition-opacity group-hover/header:opacity-100">
              <button
                type="button"
                class="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
                onclick={() => startRenameGroup(section.group)}
                title={t("repos.renameGroup")}
              >
                <Pencil class="size-3" />
              </button>
              <button
                type="button"
                class="rounded p-1 text-muted-foreground hover:bg-muted hover:text-destructive"
                onclick={() => askDeleteGroup(section.group)}
                title={t("repos.deleteGroup")}
              >
                <Trash2 class="size-3" />
              </button>
            </div>
          </div>
          <div class="overflow-hidden rounded-lg border divide-y">
            {#each section.items as entry (entry.path)}
              {@render entryRow(entry)}
            {/each}
          </div>
        </section>
      {/each}

      {#if sections.ungrouped.length > 0}
        <section>
          <h2 class="mb-2 text-xs font-medium tracking-wide text-muted-foreground uppercase">
            {t("repos.ungrouped")}
          </h2>
          <div class="overflow-hidden rounded-lg border divide-y">
            {#each sections.ungrouped as entry (entry.path)}
              {@render entryRow(entry)}
            {/each}
          </div>
        </section>
      {/if}
    {/if}
  </div>
</div>

{#snippet entryRow(entry: RepoEntry)}
  {@const bm = bookmarkColor(entry.bookmark)}
  <div
    class="group/row flex cursor-pointer items-center gap-3 px-4 py-2.5 hover:bg-accent/50 {repos.active &&
    samePath(repos.active.path, entry.path)
      ? 'bg-accent/30'
      : ''}"
    role="button"
    tabindex="0"
    onclick={() => openEntry(entry)}
    onkeydown={(e) => {
      if (e.key === "Enter") openEntry(entry);
    }}
  >
    <!-- Bookmark (colorful): icon shows when set; picker on hover. -->
    <DropdownMenu.Root>
      <DropdownMenu.Trigger>
        {#snippet child({ props })}
          <button
            {...props}
            type="button"
            class="flex size-6 shrink-0 items-center justify-center rounded {bm
              ? ''
              : 'text-muted-foreground/40 opacity-0 group-hover/row:opacity-100'} hover:bg-muted"
            title={t("repos.bookmark")}
            onclick={(e) => {
              e.stopPropagation();
              (props as { onclick?: (e: MouseEvent) => void }).onclick?.(e);
            }}
          >
            <Bookmark
              class="size-4 {bm ? 'fill-current' : ''}"
              style={bm ? `color:${bm.color}` : ''}
            />
          </button>
        {/snippet}
      </DropdownMenu.Trigger>
      <DropdownMenu.Content class="w-40">
        <div class="grid grid-cols-4 gap-1 p-1.5">
          {#each BOOKMARKS as b (b.id)}
            <button
              type="button"
              class="flex size-7 items-center justify-center rounded hover:bg-accent {entry.bookmark === b.id ? 'bg-accent' : ''}"
              title={b.id}
              onclick={() => {
                void repos.setBookmark(entry.path, b.id);
              }}
            >
              <Bookmark class="size-4 fill-current" style={`color:${b.color}`} />
            </button>
          {/each}
        </div>
        {#if entry.bookmark}
          <DropdownMenu.Separator />
          <DropdownMenu.Item onSelect={() => void repos.setBookmark(entry.path, null)}>
            {t("repos.removeBookmark")}
          </DropdownMenu.Item>
        {/if}
      </DropdownMenu.Content>
    </DropdownMenu.Root>

    <Folder class="size-4 shrink-0 text-muted-foreground" />
    <div class="min-w-0 flex-1">
      <div class="flex items-center gap-2">
        <span class="truncate text-sm">{entry.name}</span>
        {#if entry.isOpen}
          <span class="size-1.5 shrink-0 rounded-full bg-primary" title={t("tabs.open")}></span>
        {/if}
      </div>
      <div class="truncate text-[11px] text-muted-foreground">{entry.path}</div>
    </div>

    <!-- Group chip: inline re-assignment -->
    <DropdownMenu.Root>
      <DropdownMenu.Trigger>
        {#snippet child({ props })}
          <button
            {...props}
            type="button"
            class="flex shrink-0 items-center gap-0.5 rounded border px-1.5 py-0.5 text-[11px] text-muted-foreground opacity-0 group-hover/row:opacity-100 hover:bg-muted hover:text-foreground {entry.groupId
              ? 'opacity-100'
              : ''}"
            onclick={(e) => {
              e.stopPropagation();
              (props as { onclick?: (e: MouseEvent) => void }).onclick?.(e);
            }}
          >
            {entry.groupId ? (repos.groupName(entry.groupId) ?? t("repos.ungrouped")) : t("repos.group")}
            <ChevronDown class="size-3" />
          </button>
        {/snippet}
      </DropdownMenu.Trigger>
      <DropdownMenu.Content class="w-44">
        {#each repos.groups as g (g.id)}
          <DropdownMenu.Item
            disabled={entry.groupId === g.id}
            onSelect={() => void repos.setRepoGroup(entry.path, g.id)}
          >
            {g.name}
          </DropdownMenu.Item>
        {/each}
        <DropdownMenu.Separator />
        <DropdownMenu.Item onSelect={() => startCreateGroup(entry.path)}>
          {t("repos.newGroup")}
        </DropdownMenu.Item>
        {#if entry.groupId}
          <DropdownMenu.Item onSelect={() => void repos.setRepoGroup(entry.path, null)}>
            {t("repos.removeGroup")}
          </DropdownMenu.Item>
        {/if}
      </DropdownMenu.Content>
    </DropdownMenu.Root>
  </div>
{/snippet}
<Dialog.Root bind:open={promptOpen}>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>{promptTitle}</Dialog.Title>
    </Dialog.Header>
    <form
      class="space-y-4"
      onsubmit={(e) => {
        e.preventDefault();
        void submitPrompt();
      }}
    >
      <Input bind:value={promptValue} placeholder={t("repos.groupName")} />
      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (promptOpen = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={promptValue.trim().length === 0}>
          {t("common.ok")}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<!-- Delete group -->
<ConfirmDialog
  bind:open={deleteOpen}
  title={t("repos.deleteGroupTitle")}
  description={deleteTarget ? t("repos.deleteGroupConfirm", { name: deleteTarget.name }) : ""}
  confirmLabel={t("repos.deleteGroup")}
  destructive
  onconfirm={() => void confirmDeleteGroup()}
/>
