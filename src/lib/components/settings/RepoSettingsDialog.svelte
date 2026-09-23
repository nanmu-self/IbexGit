<script lang="ts">
  /**
   * 仓库设置对话框（P10）：编辑该仓库 `.git/config` 的常用键（身份 /
   * 网络代理 / 拉取推送 / 换行）。仓库级未设置时回退到全局配置 —— 输入框
   * 以"继承全局"值为占位符；清空失焦（或选择"继承"）即删除本地键。
   *
   * 保存交互与设置中心的全局常用配置一致：blur 即时保存、失败回滚重读。
   */
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import { app, net, normalizeError, type RepoConfigValue, type SshKeyInfo } from "$lib/git";
  import { showToast } from "$lib/stores/toast";

  let {
    open = $bindable(false),
    /** 目标仓库；null = 当前活动仓库（菜单 / 命令面板入口）。 */
    repoId = null,
  }: {
    open?: boolean;
    repoId?: string | null;
  } = $props();

  // ---- 文本字段：留空 = 删除本地键（回退全局） ----
  const TEXT_FIELDS = [
    { key: "user.name", labelKey: "settings.gitconfig.userName", placeholder: "IbexGit" },
    { key: "user.email", labelKey: "settings.gitconfig.userEmail", placeholder: "you@example.com" },
    {
      key: "http.proxy",
      labelKey: "settings.gitconfig.httpProxy",
      placeholder: "http://127.0.0.1:7890",
    },
    {
      key: "https.proxy",
      labelKey: "settings.gitconfig.httpsProxy",
      placeholder: "http://127.0.0.1:7890",
    },
  ] as const;

  // ---- 下拉字段："" = 继承（删除本地键），选项即允许的本地值 ----
  const SELECT_FIELDS: { key: string; labelKey: string; options: { value: string; labelKey: string }[] }[] =
    [
      {
        key: "core.autocrlf",
        labelKey: "settings.repo.autocrlf",
        options: [
          { value: "false", labelKey: "settings.repo.autocrlf.false" },
          { value: "true", labelKey: "settings.repo.autocrlf.true" },
          { value: "input", labelKey: "settings.repo.autocrlf.input" },
        ],
      },
      {
        key: "pull.rebase",
        labelKey: "settings.repo.pullRebase",
        options: [
          { value: "false", labelKey: "settings.repo.pullRebase.merge" },
          { value: "true", labelKey: "settings.repo.pullRebase.rebase" },
          { value: "merges", labelKey: "settings.repo.pullRebase.merges" },
        ],
      },
      {
        key: "fetch.prune",
        labelKey: "settings.repo.fetchPrune",
        options: [
          { value: "true", labelKey: "common.on" },
          { value: "false", labelKey: "common.off" },
        ],
      },
      {
        key: "push.autoSetupRemote",
        labelKey: "settings.repo.pushAutoSetup",
        options: [
          { value: "true", labelKey: "common.on" },
          { value: "false", labelKey: "common.off" },
        ],
      },
    ];

  let targetId = $state<string | null>(null);
  let loading = $state(false);
  let saving = $state<string | null>(null);
  let values = $state<RepoConfigValue[]>([]);
  let drafts = $state<Record<string, string>>({});

  // ---- SSH 认证（ibexgit.sshkey 三态；仅对 SSH remote 生效）----
  const SSH_KEY = "ibexgit.sshkey";
  type SshMode = "inherit" | "none" | "custom";
  let sshMode = $state<SshMode>("inherit");
  let sshPathDraft = $state("");
  let sshKeys = $state<SshKeyInfo[]>([]);

  const sshSelectOptions = $derived.by(() => {
    const opts = sshKeys
      .filter((k) => k.private_path)
      .map((k) => ({ value: k.private_path!, label: keyDisplayName(k) }));
    const cur = sshPathDraft.trim();
    // 本地已有但不在 ~/.ssh 列表里的路径（外部密钥）也列出。
    if (cur && !opts.some((o) => o.value === cur)) {
      opts.unshift({ value: cur, label: cur });
    }
    return opts;
  });

  function keyDisplayName(key: SshKeyInfo): string {
    const name = key.private_path?.split(/[\\/]/).pop() ?? key.public_path;
    return key.comment ? `${key.comment} (${name})` : name;
  }

  function pathDisplayName(path: string): string {
    return path.split(/[\\/]/).pop() ?? path;
  }

  const repoName = $derived.by(() => {
    if (!targetId) return "";
    const tab = repos.tabs.find((r) => r.id === targetId);
    return tab?.name ?? "";
  });

  function valueOf(key: string, field: "local" | "effective"): string | null {
    const v = values.find((x) => x.key === key);
    if (!v) return null;
    return field === "local" ? v.local : v.effective;
  }

  function inheritedHint(key: string): string {
    const eff = valueOf(key, "effective");
    return eff ? t("settings.repo.inherited", { value: eff }) : t("settings.repo.notSet");
  }

  /** 下拉选项；本地已有但不在预设里的值（如 pull.rebase=interactive）也列出。 */
  function selectOptions(key: string, base: { value: string; labelKey: string }[]) {
    const local = valueOf(key, "local");
    if (local && !base.some((o) => o.value === local)) {
      return [...base, { value: local, labelKey: "" }];
    }
    return base;
  }

  let lastOpen = false;
  $effect(() => {
    // 仅在 open 从 false→true 时解析目标（菜单入口取当前活动仓库），
    // 打开期间切换标签页不会改变正在编辑的仓库。
    if (open && !lastOpen) {
      targetId = repoId ?? repos.activeId;
      void load();
    }
    lastOpen = open;
  });

  async function load(): Promise<void> {
    if (!targetId) return;
    loading = true;
    try {
      const vals = await app.repoConfigValues(targetId);
      values = vals;
      sshKeys = await net.sshKeyList();
      // SSH 三态从本地值还原：null（未设）= 继承，"" = 禁用，路径 = 指定。
      const local = valueOf(SSH_KEY, "local");
      sshMode = local === null ? "inherit" : local === "" ? "none" : "custom";
      sshPathDraft = local ? local : "";
      // 保存中的字段不动草稿，避免 reload 覆盖用户正在输入的内容。
      for (const f of TEXT_FIELDS) {
        if (saving !== f.key) drafts[f.key] = valueOf(f.key, "local") ?? "";
      }
    } catch (err) {
      normalizeError(err);
      open = false;
    } finally {
      loading = false;
    }
  }

  /** Write local value（null = unset）；成功后重读（继承回退的 effective
   *  值只有后端能算准）；saving 保护当前字段草稿不被 reload 覆盖。 */
  async function save(key: string, next: string | null): Promise<void> {
    if (!targetId || saving === key) return;
    const prev = valueOf(key, "local");
    if ((next ?? "") === (prev ?? "")) return; // 未变化（含两侧都未设置）
    saving = key;
    try {
      await app.repoConfigSet(targetId, key, next);
      showToast("success", t("settings.repo.saved"));
      await load();
      drafts[key] = valueOf(key, "local") ?? ""; // trim 后的真实值
    } catch (err) {
      normalizeError(err);
      await load();
      drafts[key] = valueOf(key, "local") ?? ""; // 回滚到真实值
    } finally {
      saving = null;
    }
  }

  function saveText(key: string): void {
    void save(key, (drafts[key] ?? "").trim() === "" ? null : (drafts[key] ?? "").trim());
  }

  /** SSH 三态保存。不能用通用 save()：它的 `(next ?? "") === (prev ?? "")`
   *  会把 null（继承）和 ""（禁用）视为相等，而这两者语义不同。 */
  async function saveSsh(next: string | null): Promise<void> {
    if (!targetId || saving === SSH_KEY) return;
    if (next === valueOf(SSH_KEY, "local")) return;
    saving = SSH_KEY;
    try {
      await app.repoConfigSet(targetId, SSH_KEY, next);
      showToast("success", t("settings.repo.saved"));
      await load();
    } catch (err) {
      normalizeError(err);
      await load();
    } finally {
      saving = null;
    }
  }

  function setSshMode(mode: SshMode): void {
    if (mode === sshMode) return;
    sshMode = mode;
    if (mode === "inherit") {
      void saveSsh(null);
    } else if (mode === "none") {
      void saveSsh("");
    }
    // custom：等选/输入了路径才落盘（空路径不写入，避免误存禁用态）。
  }
</script>

<Dialog.Root bind:open>
  <!-- 宽度带 sm: 前缀：twMerge 才能压过 Dialog.Content 基类的 sm:max-w-sm。 -->
  <Dialog.Content class="max-w-[calc(100%-2rem)] sm:max-w-2xl">
    <Dialog.Header>
      <Dialog.Title>
        {repoName ? t("settings.repo.titleNamed", { name: repoName }) : t("settings.repo.title")}
      </Dialog.Title>
      <Dialog.Description>{t("settings.repo.desc")}</Dialog.Description>
    </Dialog.Header>

    {#if !targetId}
      <p class="text-sm text-muted-foreground">{t("settings.git.needRepo")}</p>
    {:else if loading}
      <div class="flex items-center justify-center py-10">
        <LoaderCircle class="size-5 animate-spin text-muted-foreground" />
      </div>
    {:else}
      <div class="max-h-[62vh] space-y-4 overflow-y-auto pr-1">
        <!-- 身份 -->
        <section class="space-y-2 rounded-md border p-3">
          <span class="text-xs font-semibold">{t("settings.repo.identity")}</span>
          <div class="grid grid-cols-2 gap-x-4 gap-y-2">
            {#each TEXT_FIELDS.slice(0, 2) as f (f.key)}
              <label class="block space-y-1">
                <span class="text-xs text-muted-foreground">{t(f.labelKey)}</span>
                <Input
                  bind:value={drafts[f.key]}
                  placeholder={valueOf(f.key, "effective") !== null
                    ? inheritedHint(f.key)
                    : f.placeholder}
                  class="h-8 font-mono text-[12px]"
                  disabled={saving === f.key}
                  onchange={() => saveText(f.key)}
                  onblur={() => saveText(f.key)}
                />
              </label>
            {/each}
          </div>
        </section>

        <!-- 网络代理 -->
        <section class="space-y-2 rounded-md border p-3">
          <span class="text-xs font-semibold">{t("settings.repo.network")}</span>
          <div class="grid grid-cols-2 gap-x-4 gap-y-2">
            {#each TEXT_FIELDS.slice(2) as f (f.key)}
              <label class="block space-y-1">
                <span class="text-xs text-muted-foreground">{t(f.labelKey)}</span>
                <Input
                  bind:value={drafts[f.key]}
                  placeholder={valueOf(f.key, "effective") !== null
                    ? inheritedHint(f.key)
                    : f.placeholder}
                  class="h-8 font-mono text-[12px]"
                  disabled={saving === f.key}
                  onchange={() => saveText(f.key)}
                  onblur={() => saveText(f.key)}
                />
              </label>
            {/each}
          </div>
        </section>

        <!-- 拉取与推送 / 换行 -->
        <section class="space-y-2 rounded-md border p-3">
          <span class="text-xs font-semibold">{t("settings.repo.workflow")}</span>
          <div class="space-y-2">
            {#each SELECT_FIELDS as f (f.key)}
              <label class="flex items-center justify-between gap-3">
                <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground" title={t(f.labelKey)}>
                  {t(f.labelKey)}
                </span>
                <select
                  class="h-8 w-64 shrink-0 rounded-md border bg-background px-2 text-[12px]"
                  disabled={saving === f.key}
                  value={valueOf(f.key, "local") ?? ""}
                  onchange={(e) => {
                    const v = e.currentTarget.value;
                    void save(f.key, v === "" ? null : v);
                  }}
                >
                  <option value="">
                    {t("settings.repo.inherit")}{valueOf(f.key, "effective")
                      ? `：${valueOf(f.key, "effective")}`
                      : ""}
                  </option>
                  {#each selectOptions(f.key, f.options) as o (o.value)}
                    <option value={o.value}>{o.labelKey ? t(o.labelKey) : o.value}</option>
                  {/each}
                </select>
              </label>
            {/each}
          </div>
          <p class="text-[11px] text-muted-foreground">{t("settings.repo.workflowHint")}</p>
        </section>

        <!-- SSH 认证（ibexgit.sshkey 三态） -->
        <section class="space-y-2 rounded-md border p-3">
          <span class="text-xs font-semibold">{t("settings.repo.ssh.title")}</span>
          <div class="grid grid-cols-3 gap-1.5">
            {#each ["inherit", "none", "custom"] as m (m)}
              <button
                type="button"
                disabled={saving === SSH_KEY}
                class="rounded-md border px-2 py-1.5 text-[12px] transition-colors {sshMode === m
                  ? 'border-primary bg-primary/10 text-foreground'
                  : 'text-muted-foreground hover:bg-accent/50'}"
                onclick={() => setSshMode(m as SshMode)}
              >
                {t(`settings.repo.ssh.mode_${m}`)}
              </button>
            {/each}
          </div>
          {#if sshMode === "custom"}
            <select
              class="h-8 w-full rounded-md border bg-background px-2 text-[12px]"
              disabled={saving === SSH_KEY}
              value={sshSelectOptions.some((o) => o.value === sshPathDraft) ? sshPathDraft : ""}
              onchange={(e) => {
                sshPathDraft = e.currentTarget.value;
                if (sshPathDraft) void saveSsh(sshPathDraft);
              }}
            >
              <option value="" disabled>{t("settings.repo.ssh.selectPlaceholder")}</option>
              {#each sshSelectOptions as o (o.value)}
                <option value={o.value}>{o.label}</option>
              {/each}
            </select>
            <Input
              bind:value={sshPathDraft}
              placeholder={t("settings.repo.ssh.customPath")}
              class="h-8 font-mono text-[12px]"
              disabled={saving === SSH_KEY}
              onchange={() => void saveSsh(sshPathDraft.trim())}
              onblur={() => void saveSsh(sshPathDraft.trim())}
            />
          {:else if sshMode === "inherit"}
            <p class="text-[11px] text-muted-foreground">
              {settings.sshKeyPath
                ? t("settings.repo.ssh.inheritHint", {
                    name: pathDisplayName(settings.sshKeyPath),
                  })
                : t("settings.repo.ssh.inheritNone")}
            </p>
          {:else}
            <p class="text-[11px] text-muted-foreground">{t("settings.repo.ssh.noneHint")}</p>
          {/if}
          <p class="text-[11px] text-muted-foreground">{t("settings.repo.ssh.hint")}</p>
        </section>
      </div>

      <Dialog.Footer>
        <Button size="sm" variant="ghost" onclick={() => (open = false)}>
          {t("common.close")}
        </Button>
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
