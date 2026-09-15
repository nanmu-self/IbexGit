<script lang="ts">
  // Branch compare (P6): pick any two refs → ahead/behind, both commit
  // lists (incoming/outgoing), and the file-level diff between them.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import DiffViewer from "$lib/components/diff/DiffViewer.svelte";
  import { t } from "$lib/i18n";
  import {
    git,
    normalizeError,
    type BranchInfo,
    type CommitInfo,
    type DiffModel,
    type TagInfo,
  } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import ArrowRightLeft from "@lucide/svelte/icons/arrow-right-left";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  let {
    open = $bindable(false),
    branches = [],
    tags = [],
    initialLeft = "",
    initialRight = "",
    repoId,
  }: {
    open?: boolean;
    branches?: BranchInfo[];
    tags?: TagInfo[];
    initialLeft?: string;
    initialRight?: string;
    repoId: string | null;
  } = $props();

  let left = $state("");
  let right = $state("");
  let ahead = $state(0);
  let behind = $state(0);
  let incoming = $state<CommitInfo[]>([]); // in right, not left
  let outgoing = $state<CommitInfo[]>([]); // in left, not right
  let files = $state<{ path: string; status: string }[]>([]);
  let selectedFile = $state<string | null>(null);
  let fileModel = $state<DiffModel | null>(null);
  let fileLoading = $state(false);
  let statsLoading = $state(false);

  $effect(() => {
    if (open) {
      left = initialLeft;
      right = initialRight;
      ahead = 0;
      behind = 0;
      incoming = [];
      outgoing = [];
      files = [];
      selectedFile = null;
      fileModel = null;
      if (left && right) void reload();
    }
  });

  const revOptions = $derived.by(() => {
    const names = [
      ...branches.map((b) => b.name),
      ...tags.map((t) => t.name),
    ];
    return [...new Set(names)].sort();
  });

  $effect(() => {
    // Reload when either ref changes while the dialog is open.
    const l = left;
    const r = right;
    if (open && l && r && (l !== initialLeft || r !== initialRight)) void reload();
  });

  async function reload(): Promise<void> {
    const id = repoId;
    if (!id || !left || !right || left === right) return;
    statsLoading = true;
    selectedFile = null;
    fileModel = null;
    try {
      const cmp = await git.branchCompare(id, left, right);
      ahead = cmp.ahead;
      behind = cmp.behind;
      const [inc, outg] = await Promise.all([
        git.revList(id, `${left}..${right}`, 200, 0),
        git.revList(id, `${right}..${left}`, 200, 0),
      ]);
      incoming = inc;
      outgoing = outg;
      const model = await git.diff(id, "commit", left, right);
      files = model.files.map((f) => ({
        path: f.new_path ?? f.old_path ?? "?",
        status: f.binary ? "B" : f.similarity !== null && f.similarity !== undefined ? "R" : "M",
      }));
    } catch (e) {
      normalizeError(e);
    } finally {
      statsLoading = false;
    }
  }

  function swap(): void {
    const l = left;
    left = right;
    right = l;
  }

  $effect(() => {
    const f = selectedFile;
    const id = repoId;
    if (!f || !id) {
      fileModel = null;
      return;
    }
    fileLoading = true;
    git
      .diff(id, "commit", left, right, [f])
      .then((m) => (fileModel = m))
      .catch((e) => normalizeError(e))
      .finally(() => (fileLoading = false));
  });

  async function copyHash(hash: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(hash);
      showToast("info", t("history.copyHash"));
    } catch {
      /* clipboard unavailable */
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-3xl">
    <Dialog.Header>
      <Dialog.Title>{t("refs.compare.title")}</Dialog.Title>
      <Dialog.Description>{t("refs.compare.desc")}</Dialog.Description>
    </Dialog.Header>

    <div class="flex items-center gap-2">
      <select bind:value={left} class="h-8 min-w-0 flex-1 rounded-md border bg-background px-2 text-[13px]">
        <option value="" disabled>{t("refs.compare.pickRef")}</option>
        {#each revOptions as r (r)}<option value={r}>{r}</option>{/each}
      </select>
      <Button variant="ghost" size="icon-sm" onclick={swap} title={t("refs.compare.swap")}>
        <ArrowRightLeft class="size-4" />
      </Button>
      <select bind:value={right} class="h-8 min-w-0 flex-1 rounded-md border bg-background px-2 text-[13px]">
        <option value="" disabled>{t("refs.compare.pickRef")}</option>
        {#each revOptions as r (r)}<option value={r}>{r}</option>{/each}
      </select>
    </div>

    {#if statsLoading}
      <div class="flex items-center justify-center gap-2 py-6 text-sm text-muted-foreground">
        <LoaderCircle class="size-4 animate-spin" /> {t("common.loading")}
      </div>
    {:else if left && right && left !== right}
      <div class="flex items-center gap-3 text-xs">
        <span class="rounded bg-blue-500/10 px-2 py-0.5 font-medium text-blue-600 dark:text-blue-400">
          {left}: ↑{ahead}
        </span>
        <span class="rounded bg-emerald-500/10 px-2 py-0.5 font-medium text-emerald-600 dark:text-emerald-400">
          {right}: ↑{behind}
        </span>
        <span class="text-muted-foreground">{t("refs.compare.mergeBaseHint")}</span>
      </div>

      <div class="grid min-h-0 grid-cols-2 gap-3">
        <div class="min-w-0">
          <div class="mb-1 text-xs font-medium text-muted-foreground">
            {t("refs.compare.outgoing", { n: outgoing.length, ref: left })}
          </div>
          <ul class="h-28 overflow-y-auto rounded-md border text-xs">
            {#each outgoing as c (c.hash)}
              <li class="flex items-center gap-2 border-b px-2 py-1 last:border-b-0">
                <button class="font-mono text-muted-foreground hover:underline" onclick={() => copyHash(c.hash)}>
                  {c.short_hash}
                </button>
                <span class="truncate">{c.message}</span>
              </li>
            {:else}
              <li class="px-2 py-2 text-muted-foreground">{t("refs.compare.none")}</li>
            {/each}
          </ul>
        </div>
        <div class="min-w-0">
          <div class="mb-1 text-xs font-medium text-muted-foreground">
            {t("refs.compare.incoming", { n: incoming.length, ref: right })}
          </div>
          <ul class="h-28 overflow-y-auto rounded-md border text-xs">
            {#each incoming as c (c.hash)}
              <li class="flex items-center gap-2 border-b px-2 py-1 last:border-b-0">
                <button class="font-mono text-muted-foreground hover:underline" onclick={() => copyHash(c.hash)}>
                  {c.short_hash}
                </button>
                <span class="truncate">{c.message}</span>
              </li>
            {:else}
              <li class="px-2 py-2 text-muted-foreground">{t("refs.compare.none")}</li>
            {/each}
          </ul>
        </div>
      </div>

      <div class="grid min-h-0 grid-cols-[180px_1fr] gap-3" style="height: 300px">
        <ul class="overflow-y-auto rounded-md border text-xs">
          {#each files as f (f.path)}
            <li>
              <button
                class="flex w-full items-center gap-1.5 border-b px-2 py-1 text-left last:border-b-0 {selectedFile === f.path ? 'bg-accent' : 'hover:bg-accent/40'}"
                onclick={() => (selectedFile = f.path)}
              >
                <span class="w-3 shrink-0 text-center font-mono text-[10px] text-muted-foreground">{f.status}</span>
                <span class="truncate" title={f.path}>{f.path}</span>
              </button>
            </li>
          {:else}
            <li class="px-2 py-2 text-muted-foreground">{t("refs.compare.noFiles")}</li>
          {/each}
        </ul>
        <div class="min-w-0 overflow-hidden rounded-md border">
          {#if selectedFile}
            <DiffViewer
              model={fileModel}
              loading={fileLoading}
              {repoId}
              ignoreWhitespace={false}
              onlineop={() => {}}
              onexpand={() => {}}
              onignorewschange={() => {}}
            />
          {:else}
            <div class="flex h-full items-center justify-center text-xs text-muted-foreground">
              {t("refs.compare.pickFile")}
            </div>
          {/if}
        </div>
      </div>
    {:else}
      <div class="py-6 text-center text-xs text-muted-foreground">{t("refs.compare.pickTwo")}</div>
    {/if}
  </Dialog.Content>
</Dialog.Root>
