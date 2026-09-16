<script lang="ts">
  /**
   * CredentialDialog (P7)：凭据请求的全局三动作对话框。
   *
   * 由 `credential://request` 事件驱动（CredentialBroker → UI 桥）：
   * - https：用户名 + 密码 + 记住（Submit / Cancel）；
   * - askpass：单行机密/文本（口令短语、用户名兜底）；
   * - host_key：SSH 指纹确认（信任并记住 / 拒绝）。
   * 用户取消 → CredentialCancelled → Runner 终止 git 进程 → 任务呈现"已取消"。
   */
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { t } from "$lib/i18n";
  import { net, type CredentialReply } from "$lib/git";
  import { pendingCredential, nextCredential } from "$lib/stores/netdialogs.svelte";

  const prompt = $derived(pendingCredential.value);
  const open = $derived(prompt !== null);

  let username = $state("");
  let secret = $state("");
  let remember = $state(true);
  let rememberTrusted = $state(true);

  // 每个 prompt 重置表单。
  $effect(() => {
    const p = prompt;
    if (p) {
      username = p.kind === "https" ? (p.username ?? "") : "";
      secret = "";
      remember = true;
    }
  });

  function respond(reply: CredentialReply): void {
    const p = prompt;
    nextCredential();
    if (p) {
      void net.credentialRespond(p.request_id, reply).catch(() => {
        // 请求已超时/失效：静默（helper 侧已按取消处理）。
      });
    }
  }

  function submitHttps(e: SubmitEvent): void {
    e.preventDefault();
    respond({
      action: "submit",
      username: username.trim() || null,
      secret: secret || null,
      remember,
    });
  }

  function submitAskpass(e: SubmitEvent): void {
    e.preventDefault();
    const p = prompt;
    if (p?.kind !== "askpass") return;
    // "Username for …" 提示取 username 字段，机密提示取 secret 字段。
    respond({
      action: "submit",
      username: p.is_secret ? null : secret || null,
      secret: p.is_secret ? secret || null : null,
      remember,
    });
  }
</script>

{#if prompt}
  <Dialog.Root open onOpenChange={(o) => !o && respond({ action: "cancel" })}>
    <Dialog.Content class="max-w-sm">
      <Dialog.Header>
        <Dialog.Title>
          {prompt.kind === "host_key"
            ? t("cred.hostkeyTitle")
            : prompt.kind === "askpass"
              ? t("cred.askpassTitle")
              : t("cred.httpsTitle")}
        </Dialog.Title>
        <Dialog.Description class="text-left">
          {#if prompt.kind === "https"}
            {t("cred.httpsDesc", { host: prompt.host })}
          {:else if prompt.kind === "host_key"}
            {t("cred.hostkeyDesc")}
          {:else}
            {t("cred.askpassDesc")}
          {/if}
        </Dialog.Description>
      </Dialog.Header>

      {#if prompt.kind === "https"}
        <form class="space-y-3" onsubmit={submitHttps}>
          <div class="rounded-md border bg-muted/40 px-2.5 py-1.5 font-mono text-[11px] break-all text-muted-foreground">
            {prompt.protocol}://{prompt.host}{prompt.path ?? ""}
          </div>
          <label class="block space-y-1">
            <span class="text-xs text-muted-foreground">{t("cred.username")}</span>
            <Input bind:value={username} autocomplete="off" class="h-8" />
          </label>
          <label class="block space-y-1">
            <span class="text-xs text-muted-foreground">{t("cred.password")}</span>
            <Input bind:value={secret} type="password" class="h-8" />
          </label>
          <label class="flex items-center gap-2 text-[13px]">
            <Checkbox bind:checked={remember} />
            {t("cred.remember")}
          </label>
          <p class="text-[11px] text-muted-foreground">{t("cred.rememberHint")}</p>
          <Dialog.Footer>
            <Button type="button" variant="ghost" size="sm" onclick={() => respond({ action: "cancel" })}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" size="sm" disabled={!secret}>{t("cred.submit")}</Button>
          </Dialog.Footer>
        </form>
      {:else if prompt.kind === "askpass"}
        <form class="space-y-3" onsubmit={submitAskpass}>
          <div class="max-h-24 overflow-y-auto rounded-md border bg-muted/40 px-2.5 py-1.5 font-mono text-[11px] break-all text-muted-foreground">
            {prompt.prompt}
          </div>
          <label class="block space-y-1">
            <span class="text-xs text-muted-foreground">
              {prompt.is_secret ? t("cred.password") : t("cred.username")}
            </span>
            <Input bind:value={secret} type={prompt.is_secret ? "password" : "text"} class="h-8" />
          </label>
          <label class="flex items-center gap-2 text-[13px]">
            <Checkbox bind:checked={remember} />
            {t("cred.rememberPassphrase")}
          </label>
          <Dialog.Footer>
            <Button type="button" variant="ghost" size="sm" onclick={() => respond({ action: "cancel" })}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" size="sm" disabled={!secret}>{t("cred.submit")}</Button>
          </Dialog.Footer>
        </form>
      {:else}
        <div class="space-y-3">
          <div class="max-h-24 overflow-y-auto rounded-md border bg-muted/40 px-2.5 py-1.5 font-mono text-[11px] break-all text-muted-foreground">
            {prompt.prompt}
          </div>
          {#if prompt.fingerprint}
            <div class="space-y-1">
              <span class="text-xs text-muted-foreground">{t("cred.fingerprint")}</span>
              <div class="rounded-md border border-warning/40 bg-warning/5 px-2.5 py-1.5 font-mono text-[12px] break-all">
                {prompt.fingerprint}
              </div>
            </div>
          {/if}
          <label class="flex items-center gap-2 text-[13px]">
            <Checkbox bind:checked={rememberTrusted} />
            {t("cred.trustRemember")}
          </label>
          <Dialog.Footer>
            <Button type="button" variant="ghost" size="sm" onclick={() => respond({ action: "cancel" })}>
              {t("cred.reject")}
            </Button>
            <Button type="button" size="sm" onclick={() => respond({ action: "trust_host_key", remember: rememberTrusted })}>
              {t("cred.trust")}
            </Button>
          </Dialog.Footer>
        </div>
      {/if}
    </Dialog.Content>
  </Dialog.Root>
{/if}
