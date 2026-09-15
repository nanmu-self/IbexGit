<script lang="ts">
  // DiffViewer consumes DiffModel only — never raw git output (ADR-003).
  // P2 ships a basic unified renderer with a hard line cap; virtual
  // scrolling, split view and Shiki highlighting arrive in P4.
  import type { DiffLine, DiffLineKind, DiffModel } from "$lib/git/bindings";
  import { t } from "$lib/i18n";
  import EmptyState from "$lib/components/ui/empty-state/EmptyState.svelte";
  import FileText from "@lucide/svelte/icons/file-text";

  let { model = null, loading = false }: { model?: DiffModel | null; loading?: boolean } =
    $props();

  /** Guardrail until P4 virtualization lands. */
  const MAX_LINES = 4000;

  interface RenderedHunk {
    header: string;
    lines: DiffLine[];
  }

  interface RenderedFile {
    path: string;
    oldPath: string | null;
    binary: boolean;
    adds: number;
    dels: number;
    hunks: RenderedHunk[];
  }

  const rendered = $derived.by<{ files: RenderedFile[]; truncated: boolean }>(() => {
    const files: RenderedFile[] = [];
    let budget = MAX_LINES;
    for (const file of model?.files ?? []) {
      let adds = 0;
      let dels = 0;
      const hunks: RenderedHunk[] = [];
      for (const hunk of file.hunks) {
        const lines: DiffLine[] = [];
        for (const line of hunk.lines) {
          if (line.kind === "add") adds += 1;
          else if (line.kind === "remove") dels += 1;
          if (budget > 0) {
            lines.push(line);
            budget -= 1;
          }
        }
        hunks.push({
          header: `@@ -${hunk.old_start},${hunk.old_count} +${hunk.new_start},${hunk.new_count} @@ ${hunk.header}`,
          lines,
        });
      }
      files.push({
        path: file.new_path ?? file.old_path ?? "",
        oldPath: file.old_path,
        binary: file.binary,
        adds,
        dels,
        hunks,
      });
    }
    return { files, truncated: budget <= 0 };
  });

  const KIND_CLASSES: Record<DiffLineKind, string> = {
    context: "",
    add: "bg-green-500/10 text-green-800 dark:text-green-300",
    remove: "bg-red-500/10 text-red-800 dark:text-red-300",
    header: "bg-cyan-500/10 text-cyan-700 dark:text-cyan-300",
  };
</script>

<div class="flex h-full min-h-0 flex-col overflow-y-auto">
  {#if loading}
    <div class="p-4 text-sm text-muted-foreground">{t("diff.loading")}</div>
  {:else if rendered.files.length === 0}
    <EmptyState
      icon={FileText}
      title={t("workspace.noFileSelected")}
      hint={t("workspace.diffHint")}
    />
  {:else}
    {#each rendered.files as file (file.path)}
      <div class="border-b last:border-b-0">
        <div class="flex items-center gap-2 bg-muted/40 px-3 py-1.5 text-xs">
          <span class="min-w-0 flex-1 truncate font-medium" title={file.path}>
            {#if file.oldPath && file.oldPath !== file.path}
              {t("workspace.rename", { old: file.oldPath, new: file.path })}
            {:else}
              {file.path}
            {/if}
          </span>
          {#if file.adds > 0}
            <span class="font-mono text-green-600 dark:text-green-400">+{file.adds}</span>
          {/if}
          {#if file.dels > 0}
            <span class="font-mono text-red-600 dark:text-red-400">-{file.dels}</span>
          {/if}
        </div>

        {#if file.binary}
          <div class="px-3 py-6 text-center text-xs text-muted-foreground">
            {t("diff.binary")} — {t("diff.binaryHint")}
          </div>
        {:else if file.hunks.length === 0}
          <div class="px-3 py-4 text-center text-xs text-muted-foreground">
            {t("diff.noChanges")}
          </div>
        {:else}
          <div class="font-mono text-xs leading-5">
            {#each file.hunks as hunk (hunk.header)}
              <div class="flex {KIND_CLASSES.header}">
                <span
                  class="w-12 shrink-0 border-r border-border/60 pr-1.5 text-right text-muted-foreground/60 select-none"
                ></span>
                <span
                  class="w-12 shrink-0 border-r border-border/60 pr-1.5 text-right text-muted-foreground/60 select-none"
                ></span>
                <span class="min-w-0 flex-1 truncate px-2">{hunk.header}</span>
              </div>
              {#each hunk.lines as line, i (i)}
                <div class="flex {KIND_CLASSES[line.kind]}">
                  <span
                    class="w-12 shrink-0 border-r border-border/60 pr-1.5 text-right text-muted-foreground/60 select-none"
                  >
                    {line.left_no ?? ""}
                  </span>
                  <span
                    class="w-12 shrink-0 border-r border-border/60 pr-1.5 text-right text-muted-foreground/60 select-none"
                  >
                    {line.right_no ?? ""}
                  </span>
                  <span class="min-w-0 flex-1 whitespace-pre-wrap break-all px-2"
                    >{line.content}</span
                  >
                </div>
              {/each}
            {/each}
          </div>
        {/if}
      </div>
    {/each}

    {#if rendered.truncated}
      <div
        class="bg-amber-500/10 px-3 py-2 text-center text-xs text-amber-700 dark:text-amber-400"
      >
        {t("diff.truncated", { lines: MAX_LINES })}
      </div>
    {/if}
  {/if}
</div>
