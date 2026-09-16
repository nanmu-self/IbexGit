<script lang="ts">
  /**
   * NetworkDialog (P7)：网络与凭据管理。
   * - 已存凭据（OS keychain 索引，不含机密）+ 删除；
   * - 已信任 SSH host key + 移除；
   * - SSH 密钥路径、代理模式（继承/不设/自定义）——经 app_set_net_config
   *   下发到 runner（spawn 时统一注入）。
   */
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { t } from "$lib/i18n";
  import { net, normalizeError, type CredentialEntry, type KnownHost } from "$lib/git";
  import { settings } from "$lib/stores/settings.svelte";
  import { showToast } from "$lib/stores/toast";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import ShieldCheck from "@lucide/svelte/icons/shield-check";
  import Globe from "@lucide/svelte/icons/globe";
  import Trash2 from "@lucide/svelte/icons/trash-2";

  let { open: dialogOpen = $bindable(false) }: { open?: boolean } = $props();

  let credentials = $state<CredentialEntry[]>([]);
  let hosts = $state<KnownHost[]>([]);
  let loading = $state(false);

  // 网络配置（表单镜像 settings store）。
  let sshKeyPath = $state("");
  let proxyMode = $state<"inherit" | "none" | "custom">("inherit");
  let proxyUrl = $state("");

  $effect(() => {
    if (dialogOpen) {
      void load();
      sshKeyPath = settings.sshKeyPath;
      proxyMode = settings.proxyMode;
      proxyUrl = settings.proxyUrl;
    }
  });

  async function load(): Promise<void> {
    loading = true;
    try {
      credentials = await net.credentialList();
      hosts = await net.knownHostsList();
    } catch (err) {
      normalizeError(err);
    } finally {
      loading = false;
    }
  }

  async function deleteCredential(key: string): Promise<void> {
    try {
      await net.credentialDelete(key);
      credentials = credentials.filter((c) => c.key !== key);
      showToast("success", t("net.credDeleted"));
    } catch (err) {
      normalizeError(err);
    }
  }

  async function removeHost(host: string): Promise<void> {
    try {
      await net.knownHostsRemove(host);
      hosts = hosts.filter((h) => h.host !== host);
      showToast("success", t("net.hostRemoved"));
    } catch (err) {
      normalizeError(err);
    }
  }

  async function saveNetConfig(): Promise<void> {
    try {
      await settings.setNetwork({
        sshKeyPath,
        proxyMode,
        proxyUrl,
      });
      showToast("success", t("net.configSaved"));
    } catch (err) {
      normalizeError(err);
    }
  }
</script>

<Dialog.Root bind:open={dialogOpen}>
  <Dialog.Content class="max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{t("net.managerTitle")}</Dialog.Title>
      <Dialog.Description>{t("net.managerDesc")}</Dialog.Description>
    </Dialog.Header>

    <div class="max-h-[60vh] space-y-5 overflow-y-auto pr-1">
      <!-- 凭据 -->
      <section class="space-y-2">
        <h3 class="flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          <KeyRound class="size-3.5" />
          {t("net.credSection")}
        </h3>
        {#if credentials.length === 0}
          <p class="rounded-md border border-dashed px-3 py-3 text-center text-xs text-muted-foreground">
            {t("net.credEmpty")}
          </p>
        {:else}
          <ul class="divide-y overflow-hidden rounded-md border">
            {#each credentials as c (c.key)}
              <li class="flex items-center gap-2 px-3 py-2">
                <div class="min-w-0 flex-1">
                  <div class="truncate text-[13px]">{c.host}</div>
                  <div class="truncate font-mono text-[11px] text-muted-foreground">
                    {c.username} · {c.key}
                  </div>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  title={t("net.credDelete")}
                  onclick={() => void deleteCredential(c.key)}
                >
                  <Trash2 class="size-3.5 text-muted-foreground" />
                </Button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <!-- SSH host key -->
      <section class="space-y-2">
        <h3 class="flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          <ShieldCheck class="size-3.5" />
          {t("net.hostSection")}
        </h3>
        {#if hosts.length === 0}
          <p class="rounded-md border border-dashed px-3 py-3 text-center text-xs text-muted-foreground">
            {t("net.hostEmpty")}
          </p>
        {:else}
          <ul class="divide-y overflow-hidden rounded-md border">
            {#each hosts as h (h.host)}
              <li class="flex items-center gap-2 px-3 py-2">
                <div class="min-w-0 flex-1">
                  <div class="truncate text-[13px]">{h.host}</div>
                  <div class="truncate font-mono text-[11px] text-muted-foreground">{h.fingerprint}</div>
                </div>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  title={t("net.hostRemove")}
                  onclick={() => void removeHost(h.host)}
                >
                  <Trash2 class="size-3.5 text-muted-foreground" />
                </Button>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <!-- 代理与 SSH -->
      <section class="space-y-3">
        <h3 class="flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          <Globe class="size-3.5" />
          {t("net.proxySection")}
        </h3>
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t("net.sshKey")}</span>
          <Input
            bind:value={sshKeyPath}
            placeholder={t("net.sshKeyHint")}
            class="h-8 font-mono text-[12px]"
          />
        </label>
        <div class="grid grid-cols-3 gap-1.5">
          {#each ["inherit", "none", "custom"] as mode}
            <button
              type="button"
              class="rounded-md border px-2 py-1.5 text-[12px] transition-colors {proxyMode === mode
                ? 'border-primary bg-primary/10 text-foreground'
                : 'text-muted-foreground hover:bg-accent/50'}"
              onclick={() => (proxyMode = mode as typeof proxyMode)}
            >
              {t(`net.proxy_${mode}`)}
            </button>
          {/each}
        </div>
        {#if proxyMode === "custom"}
          <label class="block space-y-1">
            <span class="text-xs text-muted-foreground">{t("net.proxyUrl")}</span>
            <Input bind:value={proxyUrl} placeholder="http://127.0.0.1:7890" class="h-8 font-mono text-[12px]" />
          </label>
        {:else if proxyMode === "none"}
          <label class="flex items-center gap-2 text-[13px]">
            <Checkbox checked={true} disabled />
            <span class="text-muted-foreground">{t("net.proxyNoneHint")}</span>
          </label>
        {/if}
        <div class="flex justify-end">
          <Button type="button" size="sm" onclick={() => void saveNetConfig()}>
            {t("net.configSave")}
          </Button>
        </div>
      </section>
    </div>

    <Dialog.Footer>
      <Button type="button" variant="ghost" size="sm" onclick={() => (dialogOpen = false)}>
        {t("common.close")}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
