<script lang="ts">
  /**
   * Command palette (P10, PLAN §4.6)：按 Feature 层索引全部功能，模糊搜索
   * + 最近使用。Ctrl+Shift+P 打开；功能清单来自 features.ts（与菜单、
   * keymap 同源）。
   */
  import * as Dialog from "$lib/components/ui/dialog";
  import { onAction, keymap, formatBinding, getPlatform } from "$lib/keyboard";
  import { FEATURES, type Feature } from "$lib/features";
  import { appDialogs } from "$lib/stores/appdialogs.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import { t } from "$lib/i18n";
  import Search from "@lucide/svelte/icons/search";
  import CornerDownLeft from "@lucide/svelte/icons/corner-down-left";

  const open = $derived(appDialogs.paletteOpen);
  let query = $state("");
  let selected = $state(0);
  let inputEl = $state<HTMLInputElement | null>(null);

  const platform = getPlatform();

  function shortcutLabel(f: Feature): string {
    const action = f.shortcut;
    if (!action) return "";
    const binding = keymap[action];
    return binding ? formatBinding(binding, platform) : "";
  }

  function score(query: string, target: string): number | null {
    const q = query.toLowerCase().trim();
    const s = target.toLowerCase();
    if (!q) return 0;
    const idx = s.indexOf(q);
    if (idx >= 0) return 1000 - idx * 5 - (s.length - q.length) + (idx === 0 ? 200 : 0);
    // Subsequence match (fuzzy).
    let ti = 0;
    let last = -2;
    let sc = 0;
    let gaps = 0;
    for (const ch of q) {
      const found = s.indexOf(ch, ti);
      if (found === -1) return null;
      if (found === last + 1) sc += 15;
      else {
        sc += 5;
        gaps++;
      }
      last = found;
      ti = found + 1;
    }
    return sc - gaps * 10;
  }

  const matches = $derived.by(() => {
    const hasRepo = repos.active !== null;
    const pool = FEATURES.filter((f) => !f.needsRepo || hasRepo);
    if (!query.trim()) {
      // MRU first (only still-listed features), then registry order.
      const byId = new Map(pool.map((f) => [f.id, f]));
      const recents = settings.recentFeatures
        .map((id) => byId.get(id))
        .filter((f): f is Feature => f !== undefined);
      const rest = pool
        .filter((f) => !settings.recentFeatures.includes(f.id))
        .sort((a, b) => a.group - b.group || a.order - b.order || a.section.localeCompare(b.section));
      return [...recents, ...rest];
    }
    return pool
      .map((f) => {
        const label = t(f.labelKey);
        const s =
          score(query, label) ??
          score(query, f.keywords ?? "") ??
          score(query, f.id);
        return { f, s };
      })
      .filter((x): x is { f: Feature; s: number } => x.s !== null)
      .sort((a, b) => b.s - a.s)
      .map((x) => x.f);
  });

  // Reset state whenever the palette opens; keep selection in range.
  $effect(() => {
    if (open) {
      query = "";
      selected = 0;
      queueMicrotask(() => inputEl?.focus());
    }
  });
  $effect(() => {
    void matches.length;
    if (selected >= matches.length) selected = Math.max(0, matches.length - 1);
  });

  function close(): void {
    appDialogs.paletteOpen = false;
  }

  function execute(f: Feature): void {
    void settings.pushRecentFeature(f.id);
    close();
    // Defer so the dialog teardown doesn't swallow the action's effects.
    setTimeout(() => f.run(), 0);
  }

  function onkeydown(e: KeyboardEvent): void {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      selected = (selected + 1) % Math.max(1, matches.length);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      selected = (selected - 1 + matches.length) % Math.max(1, matches.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const f = matches[selected];
      if (f) execute(f);
    }
  }

  // Global shortcut registration (also fired via features/entry points).
  $effect(() => onAction("app.palette", () => appDialogs.openPalette()));

  const sectionLabels: Record<string, string> = $derived({
    file: t("menu.file"),
    view: t("menu.view"),
    repository: t("menu.repository"),
    help: t("menu.help"),
  });
</script>

<Dialog.Root bind:open={appDialogs.paletteOpen}>
  <Dialog.Content class="max-w-xl gap-0 overflow-hidden p-0">
    <Dialog.Header class="sr-only">
      <Dialog.Title>{t("palette.title")}</Dialog.Title>
      <Dialog.Description>{t("palette.desc")}</Dialog.Description>
    </Dialog.Header>
    <div class="flex items-center gap-2 border-b px-3 py-2.5">
      <Search class="size-4 shrink-0 text-muted-foreground" />
      <!-- svelte-ignore a11y_autofocus -->
      <input
        bind:this={inputEl}
        bind:value={query}
        autofocus
        {onkeydown}
        placeholder={t("palette.placeholder")}
        class="w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
      />
      <kbd class="rounded border bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">Esc</kbd>
    </div>

    <div class="max-h-80 overflow-y-auto p-1.5" role="listbox">
      {#if matches.length === 0}
        <div class="px-3 py-8 text-center text-sm text-muted-foreground">
          {t("palette.empty")}
        </div>
      {:else}
        {#each matches as f, i (f.id)}
          <button
            type="button"
            role="option"
            aria-selected={i === selected}
            class="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm {i ===
            selected
              ? 'bg-accent text-accent-foreground'
              : 'hover:bg-accent/60'}"
            onclick={() => execute(f)}
            onmouseenter={() => (selected = i)}
          >
            <span class="min-w-0 flex-1 truncate">{t(f.labelKey)}</span>
            <span class="shrink-0 text-[10px] text-muted-foreground">
              {sectionLabels[f.section]}
            </span>
            {#if shortcutLabel(f)}
              <kbd
                class="shrink-0 rounded border bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground"
              >
                {shortcutLabel(f)}
              </kbd>
            {/if}
          </button>
        {/each}
      {/if}
    </div>

    <div
      class="flex items-center gap-3 border-t bg-muted/30 px-3 py-1.5 text-[10px] text-muted-foreground"
    >
      <span class="flex items-center gap-1">
        <CornerDownLeft class="size-3" />
        {t("palette.run")}
      </span>
      <span>{t("palette.hint")}</span>
    </div>
  </Dialog.Content>
</Dialog.Root>
