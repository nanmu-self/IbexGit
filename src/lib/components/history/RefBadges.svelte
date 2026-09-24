<script lang="ts">
  // Ref decorations from `git log --format=%d` (P5): pills for HEAD /
  // branches / remotes / tags. Decoration strings arrive pre-split, e.g.
  // "HEAD -> main", "origin/main", "tag: v1.0".
  import { t } from "$lib/i18n";
  import GitBranch from "@lucide/svelte/icons/git-branch";
  import GitBranchSlash from "@lucide/svelte/icons/git-branch";
  import Tag from "@lucide/svelte/icons/tag";
  import CircleDot from "@lucide/svelte/icons/circle-dot";

  let { refs = [] }: { refs?: string[] } = $props();

  interface Badge {
    kind: "head" | "branch" | "remote" | "tag";
    label: string;
  }

  const badges = $derived.by<Badge[]>(() => {
    const out: Badge[] = [];
    for (const raw of refs) {
      const r = raw.trim();
      if (!r) continue;
      if (r.startsWith("HEAD -> ")) {
        out.push({ kind: "head", label: r.slice(8) });
      } else if (r === "HEAD") {
        out.push({ kind: "head", label: t("history.detachedHead") });
      } else if (r.startsWith("tag: ")) {
        out.push({ kind: "tag", label: r.slice(5) });
      } else if (r.includes("/")) {
        out.push({ kind: "remote", label: r });
      } else {
        out.push({ kind: "branch", label: r });
      }
    }
    return out;
  });
</script>

<span class="inline-flex max-w-full items-center gap-1">
  {#each badges as b (b.kind + b.label)}
    {#if b.kind === "head"}
      <span
        class="inline-flex shrink-0 items-center gap-0.5 rounded border border-primary/30 bg-primary/10 px-1.5 py-px text-[10px] font-medium text-primary"
        title={b.label}
      >
        <CircleDot class="size-2.5" />
        {b.label}
      </span>
    {:else if b.kind === "branch"}
      <span
        class="inline-flex shrink-0 items-center gap-0.5 rounded border bg-muted px-1.5 py-px text-[10px] text-foreground/80"
        title={b.label}
      >
        <GitBranch class="size-2.5" />
        {b.label}
      </span>
    {:else if b.kind === "remote"}
      <span
        class="inline-flex shrink-0 items-center gap-0.5 rounded border bg-muted/60 px-1.5 py-px text-[10px] text-muted-foreground"
        title={b.label}
      >
        <GitBranchSlash class="size-2.5" />
        {b.label}
      </span>
    {:else}
      <span
        class="inline-flex shrink-0 items-center gap-0.5 rounded border border-warning/40 bg-warning-surface px-1.5 py-px text-[10px] text-warning dark:text-warning"
        title={b.label}
      >
        <Tag class="size-2.5" />
        {b.label}
      </span>
    {/if}
  {/each}
</span>
