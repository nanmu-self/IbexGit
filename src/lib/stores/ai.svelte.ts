/**
 * P11 AI 助手状态（runes store，同 netdialogs 模式）。
 *
 * - 事件接线：`wireAiEvents()` 全局订阅一次 `AiEvent`，按 taskId 路由到
 *   提交消息面板或报告对话框（两处 UI 同时只活跃一个生成任务即可，
 *   事件按 id 匹配、互不干扰）；
 * - 提交消息：preview（发送前预览）→ streaming → done（候选列表，可
 *   多方案重新生成，插入提交框后可继续编辑）；
 * - 报告：preview → streaming → done，文本供 ReportDialog 渲染/复制/导出。
 */
import { onAiEvent } from "$lib/git/events";
import type { AiPreview } from "$lib/git";
import { ai } from "$lib/git";

export type AiPhase = "idle" | "preview" | "streaming" | "done";

class AiStore {
  // ---- 提交消息面板（CommitBox 内嵌） ----
  msgOpen = $state(false);
  msgPhase = $state<AiPhase>("idle");
  msgPreview = $state<AiPreview | null>(null);
  msgText = $state("");
  msgStatus = $state("");
  msgError = $state<string | null>(null);
  /** 已生成的候选（新候选在前）；插入提交框后仍可切换。 */
  msgCandidates = $state<string[]>([]);
  #msgTaskId: number | null = null;
  #msgLanguage = "zh-CN";

  // ---- 日报/周报对话框 ----
  reportPhase = $state<AiPhase>("idle");
  reportPreview = $state<AiPreview | null>(null);
  reportText = $state("");
  reportStatus = $state("");
  reportError = $state<string | null>(null);
  #reportTaskId: number | null = null;

  // =====================
  // 提交消息
  // =====================

  openMessagePanel(language: string): void {
    this.#msgLanguage = language;
    this.msgOpen = true;
    this.msgPhase = "idle";
    this.msgError = null;
  }

  closeMessagePanel(): void {
    this.abortMessage();
    this.msgOpen = false;
  }

  /** 第一步：采集预览（不发请求）。 */
  async previewMessage(repoId: string): Promise<void> {
    this.msgPhase = "preview";
    this.msgError = null;
    this.msgPreview = null;
    try {
      this.msgPreview = await ai.previewCommitMessage(repoId);
    } catch (e) {
      this.msgError = String((e as { message?: string })?.message ?? e);
      this.msgPhase = "idle";
    }
  }

  /** 第二步：确认发送（Task 化生成）。 */
  async confirmMessage(repoId: string): Promise<void> {
    if (this.#msgTaskId !== null) return;
    this.msgPhase = "streaming";
    this.msgText = "";
    this.msgStatus = "";
    try {
      this.#msgTaskId = await ai.generateCommitMessage(repoId, this.#msgLanguage);
    } catch (e) {
      // 命令同步失败（未启用 / 配置缺失）——normalizeError 已 toast。
      this.msgError = String((e as { message?: string })?.message ?? e);
      this.msgPhase = this.msgPreview ? "preview" : "idle";
      this.#msgTaskId = null;
    }
  }

  async regenerateMessage(repoId: string): Promise<void> {
    await this.confirmMessage(repoId);
  }

  abortMessage(): void {
    if (this.#msgTaskId !== null) {
      void ai.cancel(this.#msgTaskId).catch(() => {});
      this.#msgTaskId = null;
      this.msgPhase = this.msgPreview ? "preview" : "idle";
    }
  }

  /** 事件 done 后把候选交给提交框（返回最新候选文本）。 */
  takeLatestCandidate(): string | null {
    return this.msgCandidates[0] ?? null;
  }

  // =====================
  // 日报 / 周报
  // =====================

  resetReport(): void {
    this.abortReport();
    this.reportPhase = "idle";
    this.reportPreview = null;
    this.reportText = "";
    this.reportError = null;
    this.reportStatus = "";
  }

  async previewReport(request: Parameters<typeof ai.previewReport>[0]): Promise<void> {
    this.reportPhase = "preview";
    this.reportError = null;
    this.reportPreview = null;
    try {
      this.reportPreview = await ai.previewReport(request);
    } catch (e) {
      this.reportError = String((e as { message?: string })?.message ?? e);
      this.reportPhase = "idle";
    }
  }

  async confirmReport(request: Parameters<typeof ai.generateReport>[0]): Promise<void> {
    if (this.#reportTaskId !== null) return;
    this.reportPhase = "streaming";
    this.reportText = "";
    this.reportStatus = "";
    try {
      this.#reportTaskId = await ai.generateReport(request);
    } catch (e) {
      this.reportError = String((e as { message?: string })?.message ?? e);
      this.reportPhase = this.reportPreview ? "preview" : "idle";
      this.#reportTaskId = null;
    }
  }

  abortReport(): void {
    if (this.#reportTaskId !== null) {
      void ai.cancel(this.#reportTaskId).catch(() => {});
      this.#reportTaskId = null;
      this.reportPhase = this.reportPreview ? "preview" : "idle";
    }
  }

  // =====================
  // 事件路由
  // =====================

  /** AiEvent 路由入口（wireAiEvents 订阅；按 taskId 分发到面板/对话框）。 */
  handleAiEvent(ev: {
    task_id: number;
    phase: string;
    text: string | null;
    message: string | null;
    error: { message?: string; code?: string } | null;
  }): void {
    const isMsg = ev.task_id === this.#msgTaskId;
    if (!isMsg && ev.task_id !== this.#reportTaskId) return;

    switch (ev.phase) {
      case "status":
        if (isMsg) this.msgStatus = ev.message ?? "";
        else this.reportStatus = ev.message ?? "";
        break;
      case "delta":
        if (isMsg) this.msgText += ev.text ?? "";
        else this.reportText += ev.text ?? "";
        break;
      case "done":
        if (isMsg) {
          const text = ev.text ?? "";
          if (text.trim()) {
            // 多方案：新候选在前，保留最近 5 个。
            this.msgCandidates = [text, ...this.msgCandidates].slice(0, 5);
          }
          this.msgPhase = "done";
        } else {
          this.reportText = ev.text ?? "";
          this.reportPhase = "done";
        }
        this.#clearTask(ev.task_id);
        break;
      case "cancelled":
        // 回到预览页（已看过预览）或空闲。
        if (isMsg) this.msgPhase = this.msgPreview ? "preview" : "idle";
        else this.reportPhase = this.reportPreview ? "preview" : "idle";
        this.#clearTask(ev.task_id);
        break;
      case "failed": {
        const msg =
          ev.error?.message ?? ev.message ?? "unknown error";
        if (isMsg) {
          this.msgError = msg;
          this.msgPhase = this.msgPreview ? "preview" : "idle";
        } else {
          this.reportError = msg;
          this.reportPhase = this.reportPreview ? "preview" : "idle";
        }
        this.#clearTask(ev.task_id);
        break;
      }
    }
  }

  #clearTask(taskId: number): void {
    if (this.#msgTaskId === taskId) this.#msgTaskId = null;
    if (this.#reportTaskId === taskId) this.#reportTaskId = null;
  }
}

export const aiStore = new AiStore();

// =====================
// 事件接线（模块加载即订阅；单页应用全局一份）
// =====================

let wired = false;

export function wireAiEvents(): void {
  if (wired) return;
  wired = true;
  onAiEvent((ev) => aiStore.handleAiEvent(ev));
}
