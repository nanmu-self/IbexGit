<script lang="ts">
  /**
   * ImageDiff (P4): 并排 / 滑动对比 / 差异叠加 三种模式。
   * Content arrives base64 via `git.file_content` (worktree file or a git
   * object); oversized files (>10MB, enforced Rust-side) degrade to a hint.
   */
  import { t } from "$lib/i18n";
  import { git, normalizeError } from "$lib/git";
  import type { DiffSource } from "$lib/git/bindings";

  interface Props {
    repoId: string | null;
    path: string;
    source: DiffSource;
    oldRev: string | null;
    newRev: string | null;
  }

  let { repoId, path, source, oldRev, newRev }: Props = $props();

  type Mode = "side" | "slider" | "onion";
  const MODES: Mode[] = ["side", "slider", "onion"];

  let mode = $state<Mode>("side");
  let sliderPos = $state(50);
  let onionOpacity = $state(50);

  interface Side {
    dataUrl: string | null;
    size: number;
    tooLarge: boolean;
    missing: boolean;
  }

  let oldSide = $state<Side | null>(null);
  let newSide = $state<Side | null>(null);
  let loading = $state(false);

  /** Revision spec per side: null = worktree file, "" = index stage 0. */
  const revs = $derived.by(() => {
    switch (source) {
      case "worktree":
        return { old: "" as string | null, new: null as string | null };
      case "staged":
        return { old: "HEAD" as string | null, new: "" as string | null };
      case "commit":
        return { old: (oldRev ?? null) as string | null, new: (newRev ?? null) as string | null };
      default:
        return { old: null, new: null };
    }
  });

  $effect(() => {
    const id = repoId;
    const p = path;
    const { old: ro, new: rn } = revs;
    if (!id || !p) return;
    loading = true;
    oldSide = null;
    newSide = null;
    Promise.all([fetchSide(id, p, ro), fetchSide(id, p, rn)])
      .then(([o, n]) => {
        oldSide = o;
        newSide = n;
      })
      .catch((e) => normalizeError(e))
      .finally(() => (loading = false));
  });

  async function fetchSide(
    id: string,
    p: string,
    rev: string | null,
  ): Promise<Side> {
    // rev null = worktree file, "" = index stage 0 (`:path`), otherwise a
    // revision expression (`HEAD`, a sha…). Stash has no image support.
    if (source === "stash" || (rev === null && source !== "worktree")) {
      return { dataUrl: null, size: 0, tooLarge: false, missing: true };
    }
    try {
      const fc = await git.fileContent(id, p, rev);
      if (!fc.data) return { dataUrl: null, size: fc.size, tooLarge: true, missing: false };
      const mime = guessMime(p);
      return {
        dataUrl: `data:${mime};base64,${fc.data}`,
        size: fc.size,
        tooLarge: false,
        missing: false,
      };
    } catch {
      return { dataUrl: null, size: 0, tooLarge: false, missing: true };
    }
  }

  function guessMime(path: string): string {
    const ext = path.split(".").pop()?.toLowerCase() ?? "";
    switch (ext) {
      case "png":
        return "image/png";
      case "jpg":
      case "jpeg":
        return "image/jpeg";
      case "gif":
        return "image/gif";
      case "webp":
        return "image/webp";
      case "bmp":
        return "image/bmp";
      case "svg":
        return "image/svg+xml";
      case "avif":
        return "image/avif";
      default:
        return "application/octet-stream";
    }
  }
</script>

<div class="flex h-full max-h-[340px] flex-col">
  <div class="flex items-center gap-1 border-b px-2 py-1 text-xs">
    {#each MODES as m (m)}
      <button
        class="rounded px-2 py-0.5 {mode === m
          ? 'bg-muted text-foreground'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => (mode = m)}
      >{t(`diff.image.${m}`)}</button>
    {/each}
  </div>

  {#if loading}
    <div class="flex flex-1 items-center justify-center text-xs text-muted-foreground">
      {t("common.loading")}
    </div>
  {:else if !oldSide?.dataUrl && !newSide?.dataUrl}
    <div class="flex flex-1 items-center justify-center px-4 text-center text-xs text-muted-foreground">
      {(oldSide?.tooLarge || newSide?.tooLarge)
        ? t("diff.image.tooLarge")
        : t("diff.image.missing")}
    </div>
  {:else}
    <div class="relative min-h-0 flex-1 overflow-hidden p-2">
      {#if mode === "side"}
        <div class="grid h-full grid-cols-2 gap-2">
          <figure class="flex min-h-0 flex-col items-center justify-center border bg-[repeating-conic-gradient(#8882_0%_25%,transparent_0%_50%)] bg-[length:16px_16px]">
            {#if oldSide?.dataUrl}
              <img src={oldSide.dataUrl} alt="old" class="max-h-full max-w-full object-contain" />
            {:else}
              <span class="text-xs text-muted-foreground">{t("diff.image.noOld")}</span>
            {/if}
          </figure>
          <figure class="flex min-h-0 flex-col items-center justify-center border bg-[repeating-conic-gradient(#8882_0%_25%,transparent_0%_50%)] bg-[length:16px_16px]">
            {#if newSide?.dataUrl}
              <img src={newSide.dataUrl} alt="new" class="max-h-full max-w-full object-contain" />
            {:else}
              <span class="text-xs text-muted-foreground">{t("diff.image.noNew")}</span>
            {/if}
          </figure>
        </div>
      {:else if mode === "slider" && oldSide?.dataUrl && newSide?.dataUrl}
        <div class="relative h-full">
          <img src={oldSide.dataUrl} alt="old" class="absolute inset-0 m-auto max-h-full max-w-full object-contain" />
          <div class="absolute inset-0 overflow-hidden" style="width: {sliderPos}%">
            <img src={newSide.dataUrl} alt="new" class="absolute right-0 m-auto max-h-full object-contain" style="height: 100%; max-width: none" />
          </div>
          <div class="absolute inset-y-0 w-0.5 bg-info0" style="left: {sliderPos}%"></div>
          <input
            type="range"
            class="absolute inset-x-0 bottom-1 mx-auto w-2/3"
            min="0"
            max="100"
            bind:value={sliderPos}
          />
        </div>
      {:else if oldSide?.dataUrl && newSide?.dataUrl}
        <div class="relative flex h-full items-center justify-center">
          <img src={oldSide.dataUrl} alt="old" class="max-h-full max-w-full object-contain" />
          <img
            src={newSide.dataUrl}
            alt="new"
            class="absolute max-h-full max-w-full object-contain mix-blend-difference"
            style="opacity: {onionOpacity / 100}"
          />
          <input
            type="range"
            class="absolute inset-x-0 bottom-1 mx-auto w-2/3"
            min="0"
            max="100"
            bind:value={onionOpacity}
          />
        </div>
      {:else}
        <div class="flex h-full items-center justify-center text-xs text-muted-foreground">
          {t("diff.image.missing")}
        </div>
      {/if}
    </div>
  {/if}
</div>
