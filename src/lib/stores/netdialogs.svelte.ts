/**
 * P7 网络/凭据对话框总线 + 克隆任务状态。
 *
 * - 入口（欢迎页 / 总览 Tab / 菜单 / TitleBar）通过 `openClone()` 等动作
 *   打开全局对话框（NetDialogsHost 挂载在 +page.svelte）；
 * - 克隆任务进度：`git_clone` 返回 taskId，`onCloneEvent` 事件驱动
 *   `cloneTasks` 状态；对话框与状态栏共享同一状态源。
 *
 * Svelte 5 runes: plain `$state` module object（同 netops 模式）。
 */
import { onCloneEvent, onCredentialRequest } from "$lib/git/events";
import type { CloneEvent, CredentialPrompt } from "$lib/git/events";
import { showToast } from "$lib/stores/toast";
import { t } from "$lib/i18n";
import { repos } from "$lib/stores/repos.svelte";
import { normalizeError } from "$lib/git";

export type DialogName = "clone" | "newRepo" | "credentials";

/** 一个克隆任务的展示状态。 */
export interface CloneTask {
  taskId: number;
  url: string;
  dest: string;
  phase: "running" | "done" | "cancelled" | "failed";
  stage: string | null;
  percent: number | null;
  message: string;
  error: string | null;
}

class NetDialogsStore {
  cloneOpen = $state(false);
  newRepoOpen = $state(false);
  credentialsOpen = $state(false);
  /** 活动的克隆任务（一次一个，v1 简化）。非空 ⇒ 克隆对话框切到进度页。 */
  activeTask = $state<CloneTask | null>(null);

  openClone(): void {
    this.cloneOpen = true;
  }

  openNewRepo(): void {
    this.newRepoOpen = true;
  }

  openCredentials(): void {
    this.credentialsOpen = true;
  }

  closeAll(): void {
    this.cloneOpen = false;
    this.newRepoOpen = false;
    this.credentialsOpen = false;
  }

  /** 克隆完成后的收尾：打开克隆出的仓库。 */
  async openClonedRepo(dest: string): Promise<void> {
    try {
      await repos.openPath(dest);
    } catch {
      // repos.openPath 已走 toast。
    }
  }
}

export const netDialogs = new NetDialogsStore();

// =====================
// 事件接线（模块加载即订阅；单页应用全局一份）
// =====================

let wired = false;

export function wireNetEvents(): void {
  if (wired) return;
  wired = true;

  onCloneEvent((ev: CloneEvent) => handleCloneEvent(ev));
  onCredentialRequest((p: CredentialPrompt) => {
    credentialQueue.push(p);
    // NetDialogsHost 的 $effect 监听队列变化弹出对话框。
    pendingCredential.value = p;
  });
}

function handleCloneEvent(ev: CloneEvent): void {
  const task = netDialogs.activeTask;
  if (!task || task.taskId !== ev.task_id) return;
  switch (ev.phase) {
    case "start":
    case "progress":
      task.phase = "running";
      task.stage = ev.stage;
      if (ev.percent !== null) task.percent = ev.percent;
      task.message = ev.message;
      break;
    case "done":
      task.phase = "done";
      task.percent = 100;
      task.message = ev.message;
      netDialogs.cloneOpen = false;
      showToast("success", t("net.cloneDone", { dest: task.dest }));
      void netDialogs.openClonedRepo(task.dest);
      netDialogs.activeTask = null;
      break;
    case "cancelled":
      task.phase = "cancelled";
      netDialogs.cloneOpen = false;
      showToast("info", t("net.cloneCancelled"));
      netDialogs.activeTask = null;
      break;
    case "failed": {
      task.phase = "failed";
      const err = ev.error;
      task.error =
        err && "message" in err ? err.message : ev.message;
      netDialogs.cloneOpen = false;
      normalizeError(
        err ?? { code: "internal", message: ev.message, detail: null }
      );
      netDialogs.activeTask = null;
      break;
    }
  }
}

// =====================
// 凭据请求队列（credential://request）
// =====================

/** 待展示的凭据请求（同一时刻一个；弹窗期间新请求排队）。 */
export const pendingCredential = $state<{ value: CredentialPrompt | null }>({
  value: null,
});
const credentialQueue: CredentialPrompt[] = [];

/** 凭据框关闭后取出下一个排队请求。 */
export function nextCredential(): CredentialPrompt | null {
  const next = credentialQueue.shift() ?? null;
  pendingCredential.value = next;
  return next;
}
