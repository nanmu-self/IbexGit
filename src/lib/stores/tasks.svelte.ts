/**
 * Task center store (P2 entry point only). Long-running operations become
 * Tasks in P3+ (PLAN §4.5); for now the panel shows an empty state.
 */
export interface TaskInfo {
  id: number;
  type: string;
  status: "queued" | "running" | "cancelling" | "success" | "failed" | "cancelled";
  message: string;
}

class TasksStore {
  open = $state(false);
  items = $state<TaskInfo[]>([]);

  toggle(): void {
    this.open = !this.open;
  }

  close(): void {
    this.open = false;
  }
}

export const tasks = new TasksStore();
