<script lang="ts">
  // Reset dialog (P6): soft / mixed / hard with per-mode consequence
  // description, target rev, and the recovery guarantees spelled out.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { t } from "$lib/i18n";
  import type { BranchInfo } from "$lib/git/bindings";

  let {
    open = $bindable(false),
    branches = [],
    busy = false,
    onexecute,
  }: {
    open?: boolean;
    branches?: BranchInfo[];
    busy?: boolean;
    /** Returns a promise; the dialog closes only on success. */
    onexecute: (mode: string, target: string) => Promise<void>;
  } = $props();

  let mode = $state<"soft" | "mixed" | "hard">("mixed");
  let target = $state("");
  let error = $state<string | null>(null);

  $effect(() => {
    if (open) {
      mode = "mixed";
      target = "";
      error = null;
    }
  });

  const MODE_DESC: Record<string, string> = {
    soft: "refs.reset.descSoft",
    mixed: "refs.reset.descMixed",
    hard: "refs.reset.descHard",
  };

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    const tgt = target.trim();
    if (!tgt) {
      error = t("refs.errors.required");
      return;
    }
    error = null;
    try {
      await onexecute(mode, tgt);
      open = false;
    } catch {
      // error toast already surfaced by the host
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>{t("refs.reset.title")}</Dialog.Title>
      <Dialog.Description>{t("refs.reset.desc")}</Dialog.Description>
    </Dialog.Header>
    <form class="space-y-3" onsubmit={submit}>
      <div class="space-y-1.5">
        {#each ["soft", "mixed", "hard"] as m (m)}
          <label
            class="flex cursor-pointer items-start gap-2 rounded-md border p-2.5 text-sm hover:bg-accent/50 {mode === m ? 'border-ring' : ''}"
          >
            <input type="radio" class="mt-0.5" bind:group={mode} value={m} />
            <span>
              <span class="font-medium">git reset --{m}</span>
              <span class="block text-xs text-muted-foreground">{t(MODE_DESC[m])}</span>
            </span>
          </label>
        {/each}
      </div>

      <Input
        bind:value={target}
        placeholder={t("refs.reset.targetPlaceholder")}
        class="font-mono text-[13px]"
        list="reset-targets"
      />
      <datalist id="reset-targets">
        {#each branches.slice(0, 30) as b (b.name)}
          <option value={b.name}></option>
        {/each}
      </datalist>

      {#if mode === "hard"}
        <p class="rounded-md bg-amber-500/10 px-2.5 py-2 text-xs text-amber-600 dark:text-amber-500">
          {t("refs.reset.hardWarn")}
        </p>
      {/if}
      {#if error}
        <p class="text-xs text-destructive">{error}</p>
      {/if}

      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={busy || !target.trim()} variant={mode === "hard" ? "destructive" : "default"}>
          {t("refs.reset.confirm")}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
