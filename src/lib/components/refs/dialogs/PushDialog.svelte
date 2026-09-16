<script lang="ts">
  // Push dialog (P6): remote + branch, options (set upstream, push tags)
  // and the dangerous force-with-lease guarded by a typed confirmation.
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { t } from "$lib/i18n";
  import type { RemoteInfo } from "$lib/git/bindings";
  import { settings } from "$lib/stores/settings.svelte";

  let {
    open = $bindable(false),
    remotes = [],
    branch = "",
    hasUpstream = false,
    busy = false,
    onpush,
  }: {
    open?: boolean;
    remotes?: RemoteInfo[];
    branch?: string;
    hasUpstream?: boolean;
    busy?: boolean;
    onpush: (remote: string, branch: string, o: {
      forceWithLease: boolean;
      setUpstream: boolean;
      tags: boolean;
    }) => Promise<void>;
  } = $props();

  let remote = $state("origin");
  let setUpstream = $state(false);
  let pushTags = $state(false);
  let force = $state(false);
  let confirmText = $state("");

  $effect(() => {
    if (open) {
      remote = remotes[0]?.name ?? "origin";
      // P10: defaults from the settings center.
      setUpstream =
        settings.pushSetUpstream === "always" ||
        (settings.pushSetUpstream === "whenMissing" && !hasUpstream && !!branch);
      pushTags = settings.pushIncludeTags;
      force = false;
      confirmText = "";
    }
  });

  // Force push requires typing the branch name (二次确认).
  const forceArmed = $derived(!force || confirmText.trim() === branch);

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (!branch) return;
    try {
      await onpush(remote, branch, {
        forceWithLease: force,
        setUpstream,
        tags: pushTags,
      });
      open = false;
    } catch {
      // toast surfaced by host
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>{t("refs.push.title")}</Dialog.Title>
      <Dialog.Description>{t("refs.push.desc", { branch })}</Dialog.Description>
    </Dialog.Header>
    <form class="space-y-3" onsubmit={submit}>
      <label class="block space-y-1">
        <span class="text-xs text-muted-foreground">{t("refs.push.remote")}</span>
        <select bind:value={remote} class="h-9 w-full rounded-md border bg-background px-2 text-[13px]">
          {#each remotes as r (r.name)}
            <option value={r.name}>{r.name}</option>
          {:else}
            <option value="origin">origin</option>
          {/each}
        </select>
      </label>

      <div class="space-y-1.5 text-[13px]">
        <label class="flex items-center gap-2">
          <Checkbox bind:checked={setUpstream} />
          {t("refs.push.setUpstream")}
        </label>
        <label class="flex items-center gap-2">
          <Checkbox bind:checked={pushTags} />
          {t("refs.push.tags")}
        </label>
        <label class="flex items-center gap-2">
          <Checkbox bind:checked={force} />
          <span>{t("refs.push.force")}</span>
        </label>
      </div>

      {#if force}
        <div class="space-y-1.5 rounded-md border border-destructive/40 bg-destructive/5 p-2.5">
          <p class="text-xs text-destructive">{t("refs.push.forceWarn")}</p>
          <Input
            bind:value={confirmText}
            placeholder={branch}
            class="h-8 font-mono text-[13px]"
          />
          {#if confirmText.trim() && confirmText.trim() !== branch}
            <p class="text-xs text-muted-foreground">{t("refs.push.forceMismatch", { branch })}</p>
          {/if}
        </div>
      {/if}

      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (open = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={busy || !branch || !forceArmed}>
          {t("refs.push.confirm")}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
