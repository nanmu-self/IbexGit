<script lang="ts">
  import * as ContextMenu from "$lib/components/ui/context-menu";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { pickRepo } from "$lib/repo-picker";
  import X from "@lucide/svelte/icons/x";
  import Plus from "@lucide/svelte/icons/plus";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

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
</script>

<div class="flex h-9 shrink-0 items-stretch border-b bg-muted/40 text-[13px]">
  <div class="flex min-w-0 flex-1 items-stretch overflow-x-auto">
    {#each repos.tabs as tab (tab.id)}
      <ContextMenu.Root>
        <ContextMenu.Trigger>
          {#snippet child({ props })}
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
              {#if tab.phase === "loading"}
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
        <ContextMenu.Content class="w-44">
          <ContextMenu.Item onSelect={() => void repos.close(tab.id)}>
            {t("tabs.close")}
          </ContextMenu.Item>
          <ContextMenu.Item onSelect={() => void repos.close(tab.id, true)}>
            {t("tabs.closeOthers")}
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Root>
    {/each}
    <button
      type="button"
      class="flex items-center px-3 text-muted-foreground hover:bg-background/70 hover:text-foreground"
      onclick={pickRepo}
      title={t("tabs.open")}
    >
      <Plus class="size-4" />
    </button>
  </div>
</div>
