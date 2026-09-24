<script lang="ts">
  /**
   * 设置中心 — Git 配置面板：常用项编辑（blur 即时写入全局配置）+
   * 全局/本地配置只读视图 + 全局 gitignore。
   * 仅在面板首次激活时挂载数据加载；状态在面板间切换与弹窗重开时保留。
   */
  import { Input } from "$lib/components/ui/input";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { app, normalizeError, type ConfigEntry, type GitignoreFile } from "$lib/git";
  import { showToast } from "$lib/stores/toast";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";

  let { active }: { active: boolean } = $props();

  const activeRepo = $derived(repos.active);

  let globalConfig = $state<ConfigEntry[]>([]);
  let localConfig = $state<ConfigEntry[]>([]);
  let gitignore = $state<GitignoreFile | null>(null);
  let configLoading = $state(false);

  // ---- git config 常用项编辑（写入全局配置）----
  // blur 即时保存：非空 = set，空 = unset；与 general 区字体/缩进的交互一致。
  const COMMON_CONFIG_KEYS = [
    { key: "user.name", labelKey: "settings.gitconfig.userName", placeholder: "IbexGit" },
    { key: "user.email", labelKey: "settings.gitconfig.userEmail", placeholder: "you@example.com" },
    { key: "http.proxy", labelKey: "settings.gitconfig.httpProxy", placeholder: "http://127.0.0.1:7890" },
    { key: "https.proxy", labelKey: "settings.gitconfig.httpsProxy", placeholder: "http://127.0.0.1:7890" },
  ] as const;
  let cfgEffective = $state<Record<string, string>>({});
  let cfgDrafts = $state<Record<string, string>>(
    Object.fromEntries(COMMON_CONFIG_KEYS.map((f) => [f.key, ""])),
  );
  let cfgSaving = $state<string | null>(null);

  // 激活（含弹窗重开）时重新读取。
  $effect(() => {
    if (active) void loadConfig();
  });

  async function loadConfig(): Promise<void> {
    configLoading = true;
    try {
      const [g, ig] = await Promise.all([app.configGlobal(), app.gitignoreGlobal()]);
      globalConfig = g;
      gitignore = ig;
      localConfig = activeRepo ? await app.configLocal(activeRepo.id) : [];
      syncCommonConfig(g);
    } catch (err) {
      normalizeError(err);
    } finally {
      configLoading = false;
    }
  }

  /** 同名键以最后一次出现为准（git 语义），key 比较忽略大小写。 */
  function syncCommonConfig(entries: ConfigEntry[]): void {
    const eff: Record<string, string> = {};
    for (const e of entries) {
      const lower = e.key.toLowerCase();
      if (COMMON_CONFIG_KEYS.some((f) => f.key === lower)) eff[lower] = e.value;
    }
    cfgEffective = eff;
    // 保存中的字段不动草稿，避免 reload 覆盖用户正在输入的内容。
    for (const f of COMMON_CONFIG_KEYS) {
      if (cfgSaving !== f.key) cfgDrafts[f.key] = eff[f.key] ?? "";
    }
  }

  async function saveCommonConfig(key: string): Promise<void> {
    const draft = (cfgDrafts[key] ?? "").trim();
    if (draft === (cfgEffective[key] ?? "")) return; // 未变化（含未设置过）
    cfgSaving = key;
    try {
      await app.configSetGlobal(key, draft === "" ? null : draft);
      cfgDrafts[key] = draft;
      showToast("success", t("settings.gitconfig.saved"));
      await loadConfig();
    } catch (err) {
      normalizeError(err);
      await loadConfig();
      cfgDrafts[key] = cfgEffective[key] ?? ""; // 回滚到真实值
    } finally {
      cfgSaving = null;
    }
  }
</script>

<section class="space-y-5">
  <div class="flex items-center justify-between">
    <h3 class="text-sm font-semibold">{t("settings.nav.gitconfig")}</h3>
    {#if configLoading}<LoaderCircle class="size-3.5 animate-spin" />{/if}
  </div>
  <p class="text-[11px] text-muted-foreground">{t("settings.gitconfig.readonly")}</p>

  <div class="space-y-1.5 rounded-md border p-3">
    <span class="text-xs font-medium">{t("settings.gitconfig.common")}</span>
    <div class="grid grid-cols-2 gap-x-3 gap-y-2">
      {#each COMMON_CONFIG_KEYS as f (f.key)}
        <label class="block space-y-1">
          <span class="text-xs text-muted-foreground">{t(f.labelKey)}</span>
          <Input
            bind:value={cfgDrafts[f.key]}
            placeholder={f.placeholder}
            class="h-8 font-mono text-[12px]"
            disabled={cfgSaving === f.key}
            onchange={() => void saveCommonConfig(f.key)}
          />
        </label>
      {/each}
    </div>
    <p class="text-[11px] text-muted-foreground">{t("settings.gitconfig.commonHint")}</p>
  </div>

  <div class="space-y-1.5">
    <h4 class="text-xs font-semibold text-muted-foreground">{t("settings.gitconfig.global")}</h4>
    {#if globalConfig.length === 0}
      <p class="rounded-md border border-dashed px-3 py-2 text-center text-xs text-muted-foreground">
        {t("settings.gitconfig.empty")}
      </p>
    {:else}
      <ul class="max-h-44 divide-y overflow-y-auto rounded-md border">
        {#each globalConfig as entry, i (`${entry.key}.${i}`)}
          <li class="flex gap-2 px-2.5 py-1.5 font-mono text-[11px]">
            <span class="w-44 shrink-0 truncate text-muted-foreground" title={entry.key}>{entry.key}</span>
            <span class="min-w-0 flex-1 whitespace-pre-wrap break-all">{entry.value}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  <div class="space-y-1.5">
    <h4 class="text-xs font-semibold text-muted-foreground">{t("settings.gitconfig.local")}</h4>
    {#if !activeRepo}
      <p class="text-[11px] text-muted-foreground">{t("settings.git.needRepo")}</p>
    {:else if localConfig.length === 0}
      <p class="rounded-md border border-dashed px-3 py-2 text-center text-xs text-muted-foreground">
        {t("settings.gitconfig.empty")}
      </p>
    {:else}
      <ul class="max-h-44 divide-y overflow-y-auto rounded-md border">
        {#each localConfig as entry, i (`${entry.key}.${i}`)}
          <li class="flex gap-2 px-2.5 py-1.5 font-mono text-[11px]">
            <span class="w-44 shrink-0 truncate text-muted-foreground" title={entry.key}>{entry.key}</span>
            <span class="min-w-0 flex-1 whitespace-pre-wrap break-all">{entry.value}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  <div class="space-y-1.5">
    <h4 class="text-xs font-semibold text-muted-foreground">{t("settings.gitconfig.gitignore")}</h4>
    {#if gitignore === null}
      <p class="text-[11px] text-muted-foreground">{t("settings.gitconfig.gitignoreNone")}</p>
    {:else}
      <p class="font-mono text-[11px] text-muted-foreground">{gitignore.path}</p>
      <pre
        class="editor-font max-h-36 overflow-auto rounded border bg-muted/40 p-2 text-[11px]">{gitignore.content}</pre>
    {/if}
  </div>
</section>
