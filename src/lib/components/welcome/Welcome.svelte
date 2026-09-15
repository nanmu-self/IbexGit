<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { pickRepo } from "$lib/repo-picker";
  import { showToast } from "$lib/stores/toast";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import CloudDownload from "@lucide/svelte/icons/cloud-download";
  import FilePlus from "@lucide/svelte/icons/file-plus";
  import Folder from "@lucide/svelte/icons/folder";
  import X from "@lucide/svelte/icons/x";
</script>

<div class="flex min-h-0 flex-1 items-center justify-center overflow-y-auto p-8">
  <div class="w-full max-w-md space-y-8">
    <div class="welcome-enter space-y-2 text-center">
      <div
        class="mx-auto flex size-14 items-center justify-center rounded-2xl bg-primary text-lg font-bold text-primary-foreground shadow-md"
      >
        IG
      </div>
      <h1 class="text-xl font-semibold">{t("welcome.title")}</h1>
      <p class="text-sm text-muted-foreground">{t("welcome.subtitle")}</p>
    </div>

    <div class="welcome-enter space-y-3">
      <Button class="h-11 w-full text-sm" onclick={pickRepo}>
        <FolderOpen class="size-4" />
        {t("welcome.open")}
      </Button>
      <div class="grid grid-cols-2 gap-3">
        <Button
          variant="outline"
          class="h-11 text-sm"
          onclick={() => showToast("info", t("welcome.cloneSoon"))}
        >
          <CloudDownload class="size-4" />
          {t("welcome.clone")}
        </Button>
        <Button
          variant="outline"
          class="h-11 text-sm"
          onclick={() => showToast("info", t("welcome.newSoon"))}
        >
          <FilePlus class="size-4" />
          {t("welcome.new")}
        </Button>
      </div>
    </div>

    <div class="welcome-enter">
      <div class="mb-2 text-xs font-medium tracking-wide text-muted-foreground uppercase">
        {t("welcome.recent")}
      </div>
      {#if repos.recent.length === 0}
        <div class="rounded-lg border border-dashed p-4 text-center text-xs text-muted-foreground">
          {t("welcome.recentEmpty")}
        </div>
      {:else}
        <ul class="divide-y overflow-hidden rounded-lg border">
          {#each repos.recent as r (r.path)}
            <li>
              <div
                class="group flex cursor-pointer items-center gap-2.5 px-3 py-2 hover:bg-accent/50"
                role="button"
                tabindex="0"
                onclick={() => repos.openPath(r.path)}
                onkeydown={(e) => {
                  if (e.key === "Enter") void repos.openPath(r.path);
                }}
              >
                <Folder class="size-4 shrink-0 text-muted-foreground" />
                <div class="min-w-0 flex-1">
                  <div class="truncate text-sm">{r.name}</div>
                  <div class="truncate text-[11px] text-muted-foreground">{r.path}</div>
                </div>
                <button
                  type="button"
                  class="rounded p-1 text-muted-foreground/50 opacity-0 transition-opacity hover:bg-muted hover:text-foreground group-hover:opacity-100"
                  onclick={(e) => {
                    e.stopPropagation();
                    void repos.forgetRecent(r.path);
                  }}
                  title={t("tabs.close")}
                >
                  <X class="size-3.5" />
                </button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <p class="welcome-enter text-center text-[11px] text-muted-foreground/70">{
      t("welcome.dragHint")
    }</p>
  </div>
</div>

<style>
  /* First-impression entrance (rare surface): blocks rise 8px and fade in,
   * 250ms each, staggered 40ms (total < 400ms). Reduced motion → 80ms. */
  .welcome-enter {
    transition:
      opacity 250ms ease-out,
      transform 250ms ease-out;
  }
  .welcome-enter:nth-child(2) {
    transition-delay: 40ms;
  }
  .welcome-enter:nth-child(3) {
    transition-delay: 80ms;
  }
  .welcome-enter:nth-child(4) {
    transition-delay: 120ms;
  }
  @starting-style {
    .welcome-enter {
      opacity: 0;
      transform: translateY(8px);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .welcome-enter {
      transition-duration: 80ms;
      transition-delay: 0ms;
    }
  }
</style>
