<script lang="ts">
  /**
   * 自动更新对话框（docs/auto-update-plan.md）。
   * 版本对比 → 下载进度 → 重启安装；错误可重试。视觉延续项目约定：
   * 语义令牌色、rounded-xl 覆盖层、subtle 过渡。
   */
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { t } from "$lib/i18n";
  import { appDialogs } from "$lib/stores/appdialogs.svelte";
  import { updater } from "$lib/updater/useAppUpdater.svelte";
  import Download from "@lucide/svelte/icons/download";
  import RotateCw from "@lucide/svelte/icons/rotate-cw";

  const busy = $derived(updater.status === "downloading");
  const isError = $derived(updater.status === "error");
  const isReady = $derived(updater.status === "ready");
</script>

<Dialog.Root bind:open={appDialogs.updateOpen}>
  <Dialog.Content class="max-w-sm">
    <Dialog.Header>
      <Dialog.Title>{t("update.title")}</Dialog.Title>
      <Dialog.Description>
        {t("update.desc", { version: updater.newVersion })}
      </Dialog.Description>
    </Dialog.Header>

    <!-- 版本对比 -->
    <div class="flex items-center justify-between rounded-md bg-muted/50 px-3 py-2 text-sm">
      <span class="text-muted-foreground">{t("update.current")}</span>
      <span class="font-medium tabular-nums">v{updater.currentVersion}</span>
      <RotateCw class="size-3.5 text-muted-foreground" />
      <span class="text-muted-foreground">{t("update.latest")}</span>
      <span class="font-semibold text-success tabular-nums">v{updater.newVersion}</span>
    </div>

    {#if updater.notes}
      <div class="max-h-40 overflow-y-auto rounded-md border px-3 py-2 text-sm">
        <p class="mb-1 text-xs font-medium text-muted-foreground">{t("update.notes")}</p>
        <p class="whitespace-pre-wrap">{updater.notes}</p>
      </div>
    {/if}

    <!-- 下载进度 -->
    {#if busy || isReady}
      <div class="space-y-1.5">
        <div class="flex items-center justify-between text-xs text-muted-foreground">
          <span>{isReady ? t("update.ready") : t("update.downloading")}</span>
          <span class="tabular-nums">
            {#if updater.total > 0}
              {(updater.downloaded / 1048576).toFixed(1)} / {(updater.total / 1048576).toFixed(1)} MB · {updater.progress}%
            {/if}
          </span>
        </div>
        <div
          class="h-1.5 overflow-hidden rounded-full bg-muted"
          role="progressbar"
          aria-valuenow={updater.progress}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <div
            class="h-full rounded-full bg-success transition-[width] duration-200 ease-out"
            style="width: {updater.progress}%"
          ></div>
        </div>
      </div>
    {/if}

    {#if isError}
      <p class="text-sm text-danger" role="alert">{t("update.error")}：{updater.error}</p>
    {/if}

    <Dialog.Footer>
      <Button variant="outline" onclick={() => updater.dismiss()} disabled={isReady}>
        {t("update.later")}
      </Button>
      {#if isError}
        <Button onclick={() => void updater.checkForUpdate({ silent: true })}>
          {t("update.retry")}
        </Button>
      {:else if isReady}
        <Button onclick={() => void updater.relaunchNow()}>
          {t("update.relaunch")}
        </Button>
      {:else}
        <Button disabled={busy} onclick={() => void updater.downloadAndInstall()}>
          <Download class="size-4" />
          {t("update.download")}
        </Button>
      {/if}
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
