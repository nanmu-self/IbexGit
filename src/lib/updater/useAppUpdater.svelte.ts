/**
 * 应用自动更新状态机（docs/auto-update-plan.md）。
 *
 * 官方 tauri-plugin-updater 的 runes 封装：
 *   idle → checking → available → downloading → ready → installing
 *                                     ↘ error（可重试）
 *
 * - 启动静默检查：init() 延迟 3s 调用，仅 `import.meta.env.PROD` + Tauri 环境
 *   （防 dev 模式真实请求端点/误装更新）。
 * - 手动检查：关于对话框「检查更新」按钮；无更新时 toast 提示。
 * - 有更新时 Toolbar 显示角标按钮，点击打开 UpdateDialog。
 *
 * 注意：`Update` 实例（含下载流）不放进 $state，避免被 proxy 包裹；只存可序列化的展示字段。
 */

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { showToast } from "$lib/stores/toast";
import { t } from "$lib/i18n";
import { appDialogs } from "$lib/stores/appdialogs.svelte";

export type UpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "ready"
  | "error";

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Update 对象不进 $state（防 proxy 化），仅持引用；展示字段单独存。 */
let pendingUpdate: Update | null = null;

class UpdaterStore {
  status = $state<UpdateStatus>("idle");
  /** 当前运行版本（getVersion）；取不到为 "?"。 */
  currentVersion = $state("?");
  /** 远端新版本号。 */
  newVersion = $state("");
  /** 更新说明（latest.json 的 notes）。 */
  notes = $state("");
  /** 下载进度字节。 */
  downloaded = $state(0);
  total = $state(0);
  /** 错误信息（normalizeError 之后的展示串）。 */
  error = $state("");
  /** 静默检查已完成（避免手动检查与静默检查并发重复提示）。 */
  private initialized = false;

  hasUpdate = $derived(
    this.status === "available" || this.status === "downloading" || this.status === "ready",
  );

  progress = $derived(this.total > 0 ? Math.min(100, Math.round((this.downloaded / this.total) * 100)) : 0);

  /** 应用启动时调用一次（+page.svelte）；生产环境延迟 3s 静默检查。 */
  init(): void {
    if (this.initialized) return;
    this.initialized = true;
    if (!import.meta.env.PROD || !isTauriRuntime()) return;
    setTimeout(() => void this.checkForUpdate({ silent: true }), 3000);
  }

  /**
   * 检查更新。`silent` = 静默（无更新不打扰）；否则 toast 反馈结果。
   * 返回是否发现新版本。
   */
  async checkForUpdate(options: { silent: boolean }): Promise<boolean> {
    if (this.status === "downloading" || this.status === "ready") return true;
    if (!isTauriRuntime()) {
      if (!options.silent) showToast("info", t("update.uptodate"));
      return false;
    }
    this.status = "checking";
    this.error = "";
    try {
      const update = await check();
      pendingUpdate = update;
      if (!update) {
        this.status = "idle";
        if (!options.silent) showToast("success", t("update.uptodate"));
        return false;
      }
      this.newVersion = update.version;
      this.notes = update.body ?? "";
      try {
        this.currentVersion = await getVersion();
      } catch {
        /* 保持 "?" */
      }
      this.status = "available";
      if (!options.silent) {
        // 手动检查入口在关于对话框里：发现新版时收起关于，弹出更新对话框
        appDialogs.aboutOpen = false;
        appDialogs.openUpdate();
      }
      return true;
    } catch (e) {
      this.status = "error";
      this.error = e instanceof Error ? e.message : String(e);
      if (!options.silent) showToast("error", t("update.failed"));
      return false;
    }
  }

  /** 下载并安装；完成后自动 relaunch（Windows 上 NSIS 安装器会自行关闭应用）。 */
  async downloadAndInstall(): Promise<void> {
    const update = pendingUpdate;
    if (!update || this.status !== "available") return;
    this.status = "downloading";
    this.downloaded = 0;
    this.total = 0;
    this.error = "";
    try {
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            this.total = event.data.contentLength ?? 0;
            break;
          case "Progress":
            this.downloaded += event.data.chunkLength;
            break;
          case "Finished":
            this.downloaded = this.total > 0 ? this.total : this.downloaded;
            break;
        }
      });
      this.status = "ready";
      await relaunch();
    } catch (e) {
      this.status = "error";
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  /** 下载完成后重启安装（ready 状态的按钮走这里，与首次下载分开）。 */
  async relaunchNow(): Promise<void> {
    try {
      await relaunch();
    } catch (e) {
      this.status = "error";
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  /** 关闭对话框（不打断已开始的下载，仅收起界面）。 */
  dismiss(): void {
    appDialogs.updateOpen = false;
  }
}

export const updater = new UpdaterStore();
