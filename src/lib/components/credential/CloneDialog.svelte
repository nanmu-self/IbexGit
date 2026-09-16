<script lang="ts">
  /**
   * CloneDialog (P7)：克隆远程仓库。两页式 —— 表单 → 实时进度。
   * URL 校验（https/ssh/scp/local）、目标路径选择、深度/单分支/递归子模块；
   * 进度经 CloneEvent 事件驱动（netdialogs store），取消经 clone_cancel。
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
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  let { open: dialogOpen = $bindable(false) }: { open?: boolean } = $props();

  let url = $state("");
  let dest = $state("");
  let depth = $state("");
  let singleBranch = $state(false);
  let recurseSubmodules = $state(false);
  let busy = $state(false);

  // 表单页 ↔ 进度页：activeTask 非空即进度页。
  const task = $derived(netDialogs.activeTask);
  const showProgress = $derived(task !== null);

  // 打开时重置。
  $effect(() => {
    if (dialogOpen && !showProgress) {
      url = "";
      dest = "";
      depth = "";
      singleBranch = false;
      recurseSubmodules = false;
      busy = false;
    }
  });

  function close(): void {
    // 关闭即取消进行中的克隆（进度页没有"留后台"语义，v1 简化）。
    if (task && task.phase === "running") {
      void net.cloneCancel(task.taskId).catch(() => {});
    }
    netDialogs.activeTask = null;
  }

  // URL 校验：协议前缀或 scp 简写或本地路径。
  const urlValid = $derived(
    /^(https?|ssh|git|file):\/\//.test(url) ||
      (/^[^/\s]+@[^/\s]+:/.test(url) && !url.startsWith("-")) ||
      (!url.includes("://") && /[\\/:]/.test(url) && !url.startsWith("-"))
  );

  // 从 URL 推导默认目标目录（未手选路径时）。
  function guessRepoName(): string {
    const tail = url.replace(/\/+$/, "").split(/[/:]/).pop() ?? "";
    return tail.replace(/\.git$/, "");
  }

  async function browse(): Promise<void> {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string" && picked.length > 0) {
      dest = picked;
    }
  }

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (!urlValid || busy) return;
    let target = dest.trim();
    if (!target) {
      showToast("warning", t("net.cloneNeedDest"));
      return;
    }
    busy = true;
    try {
      const taskId = await net.clone({
        url: url.trim(),
        dest: target,
        depth: depth ? Number(depth) : null,
        single_branch: singleBranch,
        recurse_submodules: recurseSubmodules,
      });
      netDialogs.activeTask = {
        taskId,
        url: url.trim(),
        dest: target,
        phase: "running",
        stage: null,
        percent: null,
        message: "",
        error: null,
      };
    } catch (err) {
      normalizeError(err);
    } finally {
      busy = false;
    }
  }

  async function cancelClone(): Promise<void> {
    if (!task) return;
    try {
      await net.cloneCancel(task.taskId);
    } catch (err) {
      // 任务可能刚好完成：忽略。
      normalizeError(err);
    }
  }
</script>

<Dialog.Root bind:open={dialogOpen}>
  <Dialog.Content class="max-w-md" showCloseButton={!showProgress}>
    {#if !showProgress}
      <Dialog.Header>
        <Dialog.Title>{t("net.cloneTitle")}</Dialog.Title>
        <Dialog.Description>{t("net.cloneDesc")}</Dialog.Description>
      </Dialog.Header>
      <form class="space-y-3" onsubmit={submit}>
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("net.cloneUrl")}</span>
          <Input bind:value={url} placeholder="https://github.com/owner/repo.git" class="h-8 font-mono text-[12px]" />
          {#if url && !urlValid}
            <span class="text-[11px] text-destructive">{t("net.cloneUrlInvalid")}</span>
          {/if}
        </label>

        <div class="space-y-1">
          <span class="text-xs text-muted-foreground">{t("net.cloneDest")}</span>
          <div class="flex gap-1.5">
            <Input bind:value={dest} placeholder={t("net.cloneDestHint")} class="h-8 flex-1 font-mono text-[12px]" />
            <Button type="button" variant="outline" size="sm" class="h-8 px-2" onclick={browse}>
              <FolderOpen class="size-3.5" />
            </Button>
          </div>
          {#if url && urlValid}
            <p class="text-[11px] text-muted-foreground">
              {t("net.cloneRepoName", { name: guessRepoName() })}
            </p>
          {/if}
        </div>

        <div class="flex items-end gap-3">
          <label class="block w-24 space-y-1">
            <span class="text-xs text-muted-foreground">{t("net.cloneDepth")}</span>
            <Input bind:value={depth} type="number" min="1" placeholder="—" class="h-8" />
          </label>
          <div class="flex-1 space-y-1.5 pb-0.5 text-[13px]">
            <label class="flex items-center gap-2">
              <Checkbox bind:checked={singleBranch} />
              {t("net.cloneSingleBranch")}
            </label>
            <label class="flex items-center gap-2">
              <Checkbox bind:checked={recurseSubmodules} />
              {t("net.cloneSubmodules")}
            </label>
          </div>
        </div>

        <Dialog.Footer>
          <Button type="button" variant="ghost" size="sm" onclick={close}>
            {t("common.cancel")}
          </Button>
          <Button type="submit" size="sm" disabled={!urlValid || busy}>
            {t("net.cloneGo")}
          </Button>
        </Dialog.Footer>
      </form>
    {:else if task}
      <Dialog.Header>
        <Dialog.Title>{t("net.cloneProgressTitle")}</Dialog.Title>
        <Dialog.Description class="font-mono text-[11px] break-all">{task.url}</Dialog.Description>
      </Dialog.Header>

      <div class="space-y-3">
        <div class="h-1.5 w-full overflow-hidden rounded-full bg-muted">
          <div
            class="h-full rounded-full bg-primary transition-all duration-300"
            class:indeterminate={task.percent === null}
            style="width: {task.percent ?? 8}%"
          ></div>
        </div>
        <div class="flex items-center gap-2 text-[12px] text-muted-foreground">
          <LoaderCircle class="size-3.5 animate-spin" />
          <span class="min-w-0 flex-1 truncate font-mono">{task.message || "…"}</span>
          {#if task.percent !== null}
            <span class="tabular-nums">{task.percent}%</span>
          {:else if task.stage}
            <span>{task.stage}</span>
          {/if}
        </div>
        <p class="text-[11px] text-muted-foreground">{t("net.cloneProgressHint")}</p>
        <Dialog.Footer>
          <Button type="button" variant="outline" size="sm" onclick={cancelClone}>
            {t("net.cloneCancel")}
          </Button>
        </Dialog.Footer>
      </div>
    {/if}
  </Dialog.Content>
</Dialog.Root>

<style>
  /* 深度未知阶段的 indeterminate 进度条（来回滑动）。 */
  .indeterminate {
    animation: indeterminate 1.2s ease-in-out infinite;
  }
  @keyframes indeterminate {
    0% {
      margin-left: 0;
      width: 8%;
    }
    50% {
      margin-left: 60%;
      width: 30%;
    }
    100% {
      margin-left: 92%;
      width: 8%;
    }
  }
</style>
