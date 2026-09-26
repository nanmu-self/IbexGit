/**
 * P12 任务中心 store：后端 TaskManager 是唯一真相源，每次生命周期变更
 * 推全量 Task 快照（TaskEvent），此处按 id upsert。面板挂在 StatusBar
 * 右下角；`wire()` 订阅事件 + 拉一次初始列表，整个会话只执行一次。
 *
 * Svelte 5 runes: class `$state` fields（同 netdialogs 模式）。
 */
import { onTaskUpdated } from "$lib/git/events";
import { tasksApi } from "$lib/git";
import type { Task, TaskStatus } from "$lib/git/bindings";

/** 前端展示上限（后端另有 200 条淘汰，这里是双保险）。 */
const MAX_ITEMS = 200;

export function isTerminal(status: TaskStatus): boolean {
  return status === "success" || status === "failed" || status === "cancelled";
}

export function isActive(status: TaskStatus): boolean {
  return !isTerminal(status);
}

class TasksStore {
  /** 面板开关（StatusBar 下拉 + Ctrl+J / 菜单 / 命令面板共享）。 */
  open = $state(false);
  /** 最新在前（upsert 置顶）。 */
  items = $state<Task[]>([]);

  #wired = false;

  /** 订阅 TaskEvent + 初始拉取（幂等；StatusBar 挂载时调用）。 */
  async wire(): Promise<void> {
    if (this.#wired) return;
    this.#wired = true;
    onTaskUpdated((ev) => this.upsert(ev.task));
    await this.reload().catch(() => {
      // 后端不可达时面板退化为空态，不阻塞 UI。
    });
  }

  async reload(): Promise<void> {
    this.items = await tasksApi.list();
  }

  upsert(task: Task): void {
    const rest = this.items.filter((t) => t.id !== task.id);
    this.items = [task, ...rest].slice(0, MAX_ITEMS);
  }

  toggle(): void {
    this.open = !this.open;
  }

  close(): void {
    this.open = false;
  }

  /** 用户取消：后端置位 CancelToken 并标记 cancelling。 */
  cancel(id: string): void {
    void tasksApi.cancel(id).catch(() => {});
  }

  async clearFinished(): Promise<void> {
    await tasksApi.clearFinished();
    await this.reload().catch(() => {});
  }

  /** 排队/进行中/取消中的任务数（状态栏角标）。 */
  get activeCount(): number {
    return this.items.filter((t) => isActive(t.status)).length;
  }

  /** 面板展示顺序：活动任务置顶，其后按新 → 旧。 */
  get sorted(): Task[] {
    return [
      ...this.items.filter((t) => isActive(t.status)),
      ...this.items.filter((t) => isTerminal(t.status)),
    ];
  }
}

export const tasks = new TasksStore();

/** StatusBar 挂载时调用：订阅 TaskEvent + 拉初始列表（幂等）。 */
export function wireTasks(): void {
  void tasks.wire();
}
