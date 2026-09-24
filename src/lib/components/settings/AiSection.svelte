<script lang="ts">
  /**
   * 设置中心 — AI 面板（P11）：提供商/模型/密钥/超时/隐私排除。
   * 仅在面板首次激活时挂载数据加载；状态在面板间切换与弹窗重开时保留。
   */
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { ai, normalizeError, type AiConfigDto } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import Trash2 from "@lucide/svelte/icons/trash-2";

  let { active }: { active: boolean } = $props();

  let aiCfg = $state<AiConfigDto | null>(null);
  let aiKeyInput = $state("");
  let aiTesting = $state(false);
  let aiTestResult = $state<string | null>(null);
  let aiSavingKey = $state(false);

  // 激活（含弹窗重开）时重新拉取配置。
  $effect(() => {
    if (active) void loadAiConfig();
  });

  async function loadAiConfig(): Promise<void> {
    try {
      aiCfg = await ai.configGet();
      aiKeyInput = "";
      aiTestResult = null;
    } catch (err) {
      normalizeError(err);
    }
  }

  function aiPatch(patch: Partial<AiConfigDto>): void {
    if (aiCfg) aiCfg = { ...aiCfg, ...patch };
  }

  async function saveAiConfig(): Promise<void> {
    if (!aiCfg) return;
    try {
      // 非空输入才更新密钥；空 = 保持不变（清除走独立按钮）。
      if (aiKeyInput.trim()) {
        aiSavingKey = true;
        await ai.setKey(aiKeyInput.trim());
        aiKeyInput = "";
        showToast("success", t("ai.settings.keySaved"));
      }
      aiCfg = await ai.configSet(aiCfg);
      showToast("success", t("ai.settings.saved"));
    } catch (err) {
      normalizeError(err);
    } finally {
      aiSavingKey = false;
    }
  }

  async function deleteAiKey(): Promise<void> {
    try {
      await ai.deleteKey();
      aiCfg = aiCfg ? { ...aiCfg, has_key: false } : aiCfg;
      showToast("success", t("ai.settings.keyDeleted"));
    } catch (err) {
      normalizeError(err);
    }
  }

  async function testAiConnection(): Promise<void> {
    if (!aiCfg) return;
    aiTesting = true;
    aiTestResult = null;
    try {
      // 先落密钥再落配置，has_key 随返回值刷新（清除/占位提示同步）。
      if (aiKeyInput.trim()) {
        await ai.setKey(aiKeyInput.trim());
        aiKeyInput = "";
      }
      aiCfg = await ai.configSet(aiCfg);
      aiTestResult = await ai.testConnection();
    } catch (err) {
      normalizeError(err);
      aiTestResult = null;
    } finally {
      aiTesting = false;
    }
  }

  const AI_PROVIDERS = [
    { id: "ollama", labelKey: "ai.settings.ollama", base: "http://localhost:11434/v1" },
    { id: "open_ai_compatible", labelKey: "ai.settings.openaiCompatible", base: "https://api.openai.com/v1" },
    { id: "anthropic", labelKey: "ai.settings.anthropic", base: "https://api.anthropic.com" },
    { id: "custom", labelKey: "ai.settings.custom", base: "" },
  ] as const;

  function setAiProvider(id: (typeof AI_PROVIDERS)[number]["id"]): void {
    if (!aiCfg) return;
    const preset = AI_PROVIDERS.find((p) => p.id === id);
    aiPatch({
      provider: id,
      base_url: preset?.base ?? "",
    });
  }

  function addCurrentRepoToExcluded(): void {
    const path = repos.active?.path;
    if (!path || !aiCfg) return;
    if (!aiCfg.excluded_repos.includes(path)) {
      aiPatch({ excluded_repos: [...aiCfg.excluded_repos, path] });
    }
  }
</script>

<section class="space-y-4">
  <h3 class="text-sm font-semibold">{t("settings.nav.ai")}</h3>
  <p class="text-[11px] text-muted-foreground">{t("ai.settings.desc")}</p>

  {#if aiCfg}
    <label class="flex items-start gap-2 rounded-md border p-3">
      <Checkbox
        checked={aiCfg.enabled}
        onCheckedChange={(v) => aiPatch({ enabled: v === true })}
      />
      <span class="space-y-0.5">
        <span class="block text-[13px] font-medium">{t("ai.settings.enable")}</span>
        <span class="block text-[11px] text-muted-foreground">{t("ai.settings.enableHint")}</span>
      </span>
    </label>

    <div class="space-y-1.5">
      <span class="text-xs text-muted-foreground">{t("ai.settings.provider")}</span>
      <div class="grid grid-cols-4 gap-1.5">
        {#each AI_PROVIDERS as p (p.id)}
          <button
            type="button"
            class="rounded-md border px-2 py-1.5 text-[11px] transition-colors {aiCfg.provider ===
            p.id
              ? 'border-primary bg-primary/10 text-foreground'
              : 'text-muted-foreground hover:bg-accent/50'}"
            onclick={() => setAiProvider(p.id)}
          >
            {t(p.labelKey)}
          </button>
        {/each}
      </div>
      {#if aiCfg.provider === "ollama"}
        <p class="text-[11px] text-success dark:text-success">{t("ai.settings.ollamaHint")}</p>
      {/if}
    </div>

    <div class="grid grid-cols-2 gap-3">
      <label class="block space-y-1">
        <span class="text-xs text-muted-foreground">
          {aiCfg.provider === "custom" ? t("ai.settings.baseUrlCustom") : t("ai.settings.baseUrl")}
        </span>
        <Input
          value={aiCfg.base_url}
          placeholder={aiCfg.provider === "custom"
            ? "https://gateway.example.com/v1/chat/completions"
            : "http://localhost:11434/v1"}
          class="h-8 font-mono text-[12px]"
          onchange={(e) => aiPatch({ base_url: e.currentTarget.value })}
        />
      </label>
      <label class="block space-y-1">
        <span class="text-xs text-muted-foreground">{t("ai.settings.model")}</span>
        <Input
          value={aiCfg.model}
          placeholder={t("ai.settings.modelHint")}
          class="h-8 font-mono text-[12px]"
          onchange={(e) => aiPatch({ model: e.currentTarget.value })}
        />
      </label>
    </div>

    <div class="grid grid-cols-2 gap-3">
      <div class="space-y-1">
        <span class="text-xs text-muted-foreground">{t("ai.settings.key")}</span>
        <div class="flex gap-1.5">
          <Input
            type="password"
            bind:value={aiKeyInput}
            placeholder={aiCfg.has_key ? t("ai.settings.keySet") : t("ai.settings.keyPlaceholder")}
            class="h-8 flex-1 font-mono text-[12px]"
            autocomplete="off"
          />
          {#if aiCfg.has_key}
            <Button type="button" variant="outline" size="sm" onclick={() => void deleteAiKey()}>
              {t("ai.settings.keyDelete")}
            </Button>
          {/if}
        </div>
        {#if aiCfg.provider === "custom"}
          <select
            value={aiCfg.custom_auth}
            class="h-8 w-full rounded-md border bg-background px-2 text-[12px]"
            onchange={(e) => aiPatch({ custom_auth: e.currentTarget.value })}
          >
            <option value="bearer">Authorization: Bearer</option>
            <option value="x_api_key">x-api-key</option>
            <option value="none">—</option>
          </select>
        {/if}
      </div>
      <label class="block space-y-1">
        <span class="text-xs text-muted-foreground">{t("ai.settings.timeout")}</span>
        <!-- value 必须与 option 的 __value 同为 number：Svelte 用
             Object.is(option.__value, value) 匹配选中项，传 String 会
             永远失配 → selectedIndex=-1 不回显 -->
        <select
          value={aiCfg.timeout_secs}
          class="h-8 w-full rounded-md border bg-background px-2 text-[13px]"
          onchange={(e) => aiPatch({ timeout_secs: Number(e.currentTarget.value) })}
        >
          {#if ![30, 60, 120, 300, 600].includes(aiCfg.timeout_secs)}
            <option value={aiCfg.timeout_secs}>{aiCfg.timeout_secs}</option>
          {/if}
          {#each [30, 60, 120, 300, 600] as s (s)}
            <option value={s}>{s}</option>
          {/each}
        </select>
      </label>
    </div>

    <label class="flex items-center gap-2 text-[13px]">
      <Checkbox
        checked={aiCfg.conventional}
        onCheckedChange={(v) => aiPatch({ conventional: v === true })}
      />
      {t("ai.settings.conventional")}
    </label>

    <div class="space-y-1.5 rounded-md border p-3">
      <span class="text-xs font-medium">{t("ai.settings.privacyTitle")}</span>
      <label class="block space-y-1">
        <span class="text-xs text-muted-foreground">{t("ai.settings.excludePatterns")}</span>
        <textarea
          rows={4}
          value={aiCfg.exclude_patterns.join("\n")}
          class="w-full resize-y rounded-md border border-input bg-transparent px-3 py-2 font-mono text-[11px] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          onchange={(e) =>
            aiPatch({
              exclude_patterns: e.currentTarget.value
                .split("\n")
                .map((s) => s.trim())
                .filter((s) => s.length > 0),
            })}
        ></textarea>
        <p class="text-[11px] text-muted-foreground">{t("ai.settings.excludeHint")}</p>
      </label>
      <div class="space-y-1">
        <div class="flex items-center justify-between">
          <span class="text-xs text-muted-foreground">{t("ai.settings.excludedRepos")}</span>
          {#if repos.active}
            <Button type="button" variant="ghost" size="sm" onclick={addCurrentRepoToExcluded}>
              {t("ai.settings.addCurrentRepo")}
            </Button>
          {:else}
            <span class="text-[11px] text-muted-foreground">{t("ai.settings.noRepo")}</span>
          {/if}
        </div>
        {#if aiCfg.excluded_repos.length > 0}
          <ul class="divide-y overflow-hidden rounded-md border">
            {#each aiCfg.excluded_repos as rp (rp)}
              <li class="flex items-center gap-2 px-2.5 py-1.5">
                <span class="min-w-0 flex-1 truncate font-mono text-[11px]" title={rp}>{rp}</span>
                <button
                  type="button"
                  class="text-muted-foreground hover:text-foreground"
                  onclick={() =>
                    aiPatch({ excluded_repos: aiCfg!.excluded_repos.filter((x) => x !== rp) })}
                  aria-label={t("ai.settings.keyDelete")}
                >
                  <Trash2 class="size-3.5" />
                </button>
              </li>
            {/each}
          </ul>
        {/if}
        <p class="text-[11px] text-muted-foreground">{t("ai.settings.excludedReposHint")}</p>
      </div>
    </div>

    <div class="flex items-center gap-2">
      <Button type="button" variant="outline" size="sm" disabled={aiTesting} onclick={() => void testAiConnection()}>
        {#if aiTesting}<LoaderCircle class="size-3.5 animate-spin" data-icon="inline-start" />{/if}
        {t("ai.settings.test")}
      </Button>
      {#if aiTestResult}
        <span class="truncate font-mono text-[11px] text-success dark:text-success">{aiTestResult}</span>
      {/if}
    </div>

    <div class="flex justify-end">
      <Button type="button" size="sm" disabled={aiSavingKey} onclick={() => void saveAiConfig()}>
        {t("ai.settings.save")}
      </Button>
    </div>
  {:else}
    <p class="text-[11px] text-muted-foreground"><LoaderCircle class="mr-1 inline size-3 animate-spin" />…</p>
  {/if}
</section>
