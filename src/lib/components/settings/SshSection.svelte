<script lang="ts">
  /**
   * 设置中心 — 凭据面板（SSH 密钥管理：~/.ssh 列出 / 生成 / 删除 / 活动
   * 密钥切换），含生成、公钥查看、删除确认三个子对话框。
   * 仅在面板首次激活时挂载数据加载；状态在面板间切换与弹窗重开时保留。
   */
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { t } from "$lib/i18n";
  import { settings } from "$lib/stores/settings.svelte";
  import { net, normalizeError, type SshKeyInfo } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import { ConfirmDialog } from "$lib/components/ui/confirm-dialog";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Lock from "@lucide/svelte/icons/lock";
  import Copy from "@lucide/svelte/icons/copy";
  import Plus from "@lucide/svelte/icons/plus";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import Eye from "@lucide/svelte/icons/eye";

  let { active }: { active: boolean } = $props();

  // ---- SSH 密钥管理（~/.ssh 列出 / 生成 / 删除） ----
  let sshKeys = $state<SshKeyInfo[]>([]);
  let sshLoading = $state(false);
  // 生成对话框
  let genOpen = $state(false);
  let genAlgo = $state<"ed25519" | "rsa4096">("ed25519");
  let genFileName = $state("");
  let genComment = $state("");
  let genPass = $state("");
  let genPass2 = $state("");
  let genBusy = $state(false);
  // 删除确认 / 公钥查看
  let deleteTarget = $state<SshKeyInfo | null>(null);
  let deleteOpen = $state(false);
  let pubOpen = $state(false);
  let pubView = $state<SshKeyInfo | null>(null);

  // 激活（含弹窗重开）时刷新列表。
  $effect(() => {
    if (active) void loadSshKeys();
  });

  async function loadSshKeys(): Promise<void> {
    sshLoading = true;
    try {
      sshKeys = await net.sshKeyList();
    } catch (err) {
      normalizeError(err);
    } finally {
      sshLoading = false;
    }
  }

  function openGenerate(): void {
    genAlgo = "ed25519";
    genFileName = "";
    genComment = "";
    genPass = "";
    genPass2 = "";
    genOpen = true;
  }

  function keyFileName(key: SshKeyInfo): string {
    return key.public_path.split(/[\\/]/).pop() ?? key.public_path;
  }

  async function generateSshKey(): Promise<void> {
    genBusy = true;
    try {
      const info = await net.sshKeyGenerate({
        algorithm:
          genAlgo === "ed25519" ? { kind: "ed25519" } : { kind: "rsa", bits: 4096 },
        comment: genComment.trim() || null,
        passphrase: genPass || null,
        file_name: genFileName.trim() || null,
      });
      genOpen = false;
      // 首把密钥自动设为活动：未指定过密钥时生成即启用（此刻意图最明确，
      // 已有指定则不覆盖；活动密钥可随时在列表一键切换/撤销）。
      if (!settings.sshKeyPath && info.private_path) {
        await settings.setSshKeyPath(info.private_path);
        showToast("success", t("settings.ssh.generatedActive", { name: keyFileName(info) }));
      } else {
        showToast("success", t("settings.ssh.generated", { name: keyFileName(info) }));
      }
      await loadSshKeys();
    } catch (err) {
      normalizeError(err);
    } finally {
      genBusy = false;
    }
  }

  function setActiveKey(key: SshKeyInfo): void {
    if (!key.private_path) return;
    settings
      .setSshKeyPath(key.private_path)
      .then(() => showToast("success", t("settings.ssh.activeSet")))
      .catch(normalizeError);
  }

  /** 取消活动密钥（空 = 不再注入 GIT_SSH_COMMAND，认证交回系统 ssh 配置）。 */
  function clearActiveKey(): void {
    settings
      .setSshKeyPath("")
      .then(() => showToast("info", t("settings.ssh.activeUnset")))
      .catch(normalizeError);
  }

  async function confirmDeleteKey(): Promise<void> {
    const key = deleteTarget;
    deleteTarget = null;
    if (!key) return;
    try {
      await net.sshKeyDelete(key.private_path ?? key.public_path);
      if (key.private_path && settings.sshKeyPath === key.private_path) {
        // 删除的正是活动密钥：清空并停用注入。
        await settings.setSshKeyPath("");
        showToast("info", t("settings.ssh.activeCleared"));
      }
      showToast("success", t("settings.ssh.deleted", { name: keyFileName(key) }));
      await loadSshKeys();
    } catch (err) {
      normalizeError(err);
    }
  }

  function copyPubKey(key: SshKeyInfo): void {
    navigator.clipboard
      ?.writeText(key.public_key)
      .then(() => showToast("success", t("settings.ssh.copied")))
      .catch(() => {});
  }
</script>

<section class="space-y-4">
  <div class="flex items-center justify-between">
    <h3 class="text-sm font-semibold">{t("settings.nav.credentials")}</h3>
    <div class="flex items-center gap-2">
      {#if sshLoading}<LoaderCircle class="size-3.5 animate-spin" />{/if}
      <Button type="button" size="sm" onclick={openGenerate}>
        <Plus class="size-3.5" data-icon="inline-start" />
        {t("settings.ssh.generate")}
      </Button>
    </div>
  </div>
  <p class="text-[11px] text-muted-foreground">{t("settings.ssh.dirHint")}</p>

  {#if sshKeys.length === 0}
    <p class="rounded-md border border-dashed px-3 py-6 text-center text-xs text-muted-foreground">
      {t("settings.ssh.empty")}
    </p>
  {:else}
    <ul class="divide-y overflow-hidden rounded-md border">
      {#each sshKeys as key (key.public_path)}
        <li class="flex items-center gap-3 px-3 py-2.5">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-1.5">
              <span class="truncate text-[13px] font-medium">
                {key.comment || keyFileName(key)}
              </span>
              {#if key.encrypted}
                <Lock class="size-3 shrink-0 text-muted-foreground" aria-label={t("settings.ssh.encrypted")} />
              {/if}
              {#if key.private_path && settings.sshKeyPath === key.private_path}
                <span class="shrink-0 rounded bg-primary/10 px-1.5 py-0.5 text-[10px] font-medium text-primary">
                  {t("settings.ssh.active")}
                </span>
              {/if}
            </div>
            <div class="truncate font-mono text-[11px] text-muted-foreground" title={key.public_path}>
              {key.algorithm}{key.bits > 0 ? ` · ${key.bits}` : ""} · {key.fingerprint} · {key.public_path}
            </div>
          </div>
          <div class="flex shrink-0 items-center gap-0.5">
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title={t("settings.ssh.viewPub")}
              onclick={() => {
                pubView = key;
                pubOpen = true;
              }}
            >
              <Eye class="size-3.5 text-muted-foreground" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title={t("settings.ssh.copyPub")}
              onclick={() => copyPubKey(key)}
            >
              <Copy class="size-3.5 text-muted-foreground" />
            </Button>
            {#if key.private_path && settings.sshKeyPath !== key.private_path}
              <Button
                type="button"
                variant="ghost"
                size="sm"
                title={t("settings.ssh.setActiveHint")}
                onclick={() => setActiveKey(key)}
              >
                {t("settings.ssh.setActive")}
              </Button>
            {:else if key.private_path}
              <Button
                type="button"
                variant="ghost"
                size="sm"
                title={t("settings.ssh.unsetActiveHint")}
                onclick={clearActiveKey}
              >
                {t("settings.ssh.unsetActive")}
              </Button>
            {/if}
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              title={t("settings.ssh.deleteKey")}
              onclick={() => {
                deleteTarget = key;
                deleteOpen = true;
              }}
            >
              <Trash2 class="size-3.5 text-muted-foreground" />
            </Button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
  <p class="text-[11px] text-muted-foreground">{t("settings.ssh.activeHint")}</p>
</section>

<!-- SSH 密钥生成（嵌套在设置中心之上） -->
<Dialog.Root bind:open={genOpen}>
  <Dialog.Content class="max-w-md">
    <Dialog.Header>
      <Dialog.Title>{t("settings.ssh.genTitle")}</Dialog.Title>
      <Dialog.Description>{t("settings.ssh.genDesc")}</Dialog.Description>
    </Dialog.Header>
    <div class="space-y-3">
      <div class="space-y-1.5">
        <span class="text-xs text-muted-foreground">{t("settings.ssh.algorithm")}</span>
        <div class="grid grid-cols-2 gap-1.5">
          {#each [["ed25519", "settings.ssh.algoEd25519", "settings.ssh.algoEd25519Hint"], ["rsa4096", "settings.ssh.algoRsa", "settings.ssh.algoRsaHint"]] as [id, label, hint]}
            <button
              type="button"
              class="rounded-md border px-2 py-1.5 text-left text-[12px] transition-colors {genAlgo === id
                ? 'border-primary bg-primary/10 text-foreground'
                : 'text-muted-foreground hover:bg-accent/50'}"
              onclick={() => (genAlgo = id as typeof genAlgo)}
            >
              {t(label)}
              <span class="block text-[10px] opacity-80">{t(hint)}</span>
            </button>
          {/each}
        </div>
      </div>
      <div class="grid grid-cols-2 gap-3">
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("settings.ssh.fileName")}</span>
          <Input
            bind:value={genFileName}
            placeholder={genAlgo === "ed25519" ? "id_ed25519" : "id_rsa"}
            class="h-8 font-mono text-[12px]"
          />
        </label>
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("settings.ssh.comment")}</span>
          <Input
            bind:value={genComment}
            placeholder="you@example.com"
            class="h-8 text-[12px]"
          />
        </label>
      </div>
      <div class="grid grid-cols-2 gap-3">
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("settings.ssh.passphrase")}</span>
          <Input
            type="password"
            bind:value={genPass}
            placeholder={t("settings.ssh.passphraseHint")}
            class="h-8 font-mono text-[12px]"
            autocomplete="new-password"
          />
        </label>
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("settings.ssh.passphraseConfirm")}</span>
          <Input
            type="password"
            bind:value={genPass2}
            class="h-8 font-mono text-[12px]"
            autocomplete="new-password"
          />
        </label>
      </div>
      {#if genPass !== genPass2}
        <p class="text-[11px] text-destructive">{t("settings.ssh.passMismatch")}</p>
      {/if}
      <p class="text-[11px] text-muted-foreground">{t("settings.ssh.passphraseNote")}</p>
    </div>
    <Dialog.Footer>
      <Button type="button" variant="ghost" size="sm" disabled={genBusy} onclick={() => (genOpen = false)}>
        {t("common.cancel")}
      </Button>
      <Button
        type="button"
        size="sm"
        disabled={genBusy || (genPass.length > 0 && genPass !== genPass2)}
        onclick={() => void generateSshKey()}
      >
        {#if genBusy}<LoaderCircle class="size-3.5 animate-spin" data-icon="inline-start" />{/if}
        {t("settings.ssh.generate")}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- 公钥查看 / 复制 -->
<Dialog.Root bind:open={pubOpen}>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{t("settings.ssh.pubTitle")}</Dialog.Title>
      {#if pubView}
        <Dialog.Description class="font-mono text-[11px]">
          {pubView.algorithm}{pubView.bits > 0 ? ` · ${pubView.bits}` : ""} · {pubView.fingerprint}
        </Dialog.Description>
      {/if}
    </Dialog.Header>
    {#if pubView}
      <pre
        class="editor-font max-h-40 overflow-auto rounded border bg-muted/40 p-2 text-[11px] break-all whitespace-pre-wrap">{pubView.public_key}</pre>
    {/if}
    <Dialog.Footer>
      <Button type="button" variant="ghost" size="sm" onclick={() => (pubOpen = false)}>
        {t("common.close")}
      </Button>
      {#if pubView}
        <Button type="button" size="sm" onclick={() => copyPubKey(pubView!)}>
          <Copy class="size-3.5" data-icon="inline-start" />
          {t("settings.ssh.copyPub")}
        </Button>
      {/if}
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>

<!-- 删除确认（破坏性操作：私钥 + 公钥同时移除，不可恢复） -->
<ConfirmDialog
  bind:open={deleteOpen}
  title={deleteTarget ? t("settings.ssh.deleteTitle", { name: keyFileName(deleteTarget) }) : ""}
  description={t("settings.ssh.deleteDesc")}
  confirmLabel={t("settings.ssh.deleteKey")}
  destructive
  onconfirm={() => void confirmDeleteKey()}
/>
