<script lang="ts">
  /**
   * NewRepoDialog (P7)：新建空仓库。`git init`（默认分支 main）+
   * 可选 README / .gitignore 模板。模板文件不自动提交（避免替用户
   * 伪造 author 身份），留给工作区提交框完成。
   */
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { open } from "@tauri-apps/plugin-dialog";
  import { t } from "$lib/i18n";
  import { net, normalizeError } from "$lib/git";
  import { netDialogs } from "$lib/stores/netdialogs.svelte";
  import { showToast } from "$lib/stores/toast";
  import { repos } from "$lib/stores/repos.svelte";
  import FolderOpen from "@lucide/svelte/icons/folder-open";

  let { open: dialogOpen = $bindable(false) }: { open?: boolean } = $props();

  let parentDir = $state("");
  let name = $state("");
  let readme = $state(true);
  let gitignore = $state(false);
  let busy = $state(false);

  $effect(() => {
    if (dialogOpen) {
      parentDir = "";
      name = "";
      readme = true;
      gitignore = false;
      busy = false;
    }
  });

  const valid = $derived(parentDir.trim().length > 0 && name.trim().length > 0);

  async function browse(): Promise<void> {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string" && picked.length > 0) {
      parentDir = picked;
    }
  }

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (!valid || busy) return;
    const path = parentDir.trim() + sep() + name.trim();
    busy = true;
    try {
      await net.initRepo(path, readme, gitignore);
      dialogOpen = false;
      netDialogs.newRepoOpen = false;
      showToast("success", t("net.initDone", { path }));
      await repos.openPath(path);
    } catch (err) {
      normalizeError(err);
    } finally {
      busy = false;
    }
  }

  function sep(): string {
    return parentDir.includes("\\") || navigator.userAgent.includes("Windows") ? "\\" : "/";
  }
</script>

<Dialog.Root bind:open={dialogOpen}>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>{t("net.initTitle")}</Dialog.Title>
      <Dialog.Description>{t("net.initDesc")}</Dialog.Description>
    </Dialog.Header>
    <form class="space-y-3" onsubmit={submit}>
      <div class="space-y-1">
        <span class="text-xs text-muted-foreground">{t("net.initParent")}</span>
        <div class="flex gap-1.5">
          <Input bind:value={parentDir} placeholder="C:\repos" class="h-8 flex-1 font-mono text-[12px]" />
          <Button type="button" variant="outline" size="sm" class="h-8 px-2" onclick={browse}>
            <FolderOpen class="size-3.5" />
          </Button>
        </div>
      </div>
      <label class="block space-y-1">
        <span class="text-xs text-muted-foreground">{t("net.initName")}</span>
        <Input bind:value={name} placeholder="my-project" class="h-8" />
      </label>
      <div class="space-y-1.5 text-[13px]">
        <label class="flex items-center gap-2">
          <Checkbox bind:checked={readme} />
          {t("net.initReadme")}
        </label>
        <label class="flex items-center gap-2">
          <Checkbox bind:checked={gitignore} />
          {t("net.initGitignore")}
        </label>
      </div>
      {#if valid}
        <p class="font-mono text-[11px] break-all text-muted-foreground">{parentDir + sep() + name}</p>
      {/if}
      <Dialog.Footer>
        <Button type="button" variant="ghost" size="sm" onclick={() => (dialogOpen = false)}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" size="sm" disabled={!valid || busy}>{t("net.initGo")}</Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
