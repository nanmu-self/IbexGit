import { events, type RepoChanged, type AppOpenPaths, type CloneEvent, type CredentialPrompt, type AiEvent, type TaskEvent } from "./bindings";

export type { RepoChanged } from "./bindings";
export type { UnlistenFn } from "@tauri-apps/api/event";
export type { CloneEvent, CredentialPrompt, AiEvent, TaskEvent } from "./bindings";

/**
 * Subscribe to repository change events (state invalidation system,
 * PLAN §4.3): emitted by the backend after a change was detected, debounced,
 * and the caches re-read.
 */
export function onRepoChanged(
  handler: (payload: RepoChanged) => void
): Promise<() => void> {
  return events.repoChanged.listen((e) => handler(e.payload));
}

export type { AppOpenPaths } from "./bindings";

/**
 * Emitted when a second app instance was launched with repo paths
 * (single-instance plugin); the primary instance should open them.
 */
export function onAppOpenPaths(
  handler: (payload: AppOpenPaths) => void
): Promise<() => void> {
  return events.appOpenPaths.listen((e) => handler(e.payload));
}

/**
 * P7 克隆任务进度事件（CloneEvent）：`git_clone` 后台任务推送，
 * phase = start | progress | done | cancelled | failed。
 */
export function onCloneEvent(
  handler: (payload: CloneEvent) => void
): Promise<() => void> {
  return events.cloneEvent.listen((e) => handler(e.payload));
}

/**
 * P12 任务中心事件（TaskEvent）：TaskManager 每次生命周期变更推送全量
 * Task 快照，前端按 id upsert。
 */
export function onTaskUpdated(handler: (payload: TaskEvent) => void): Promise<() => void> {
  return events.taskEvent.listen((e) => handler(e.payload));
}

/**
 * P11 AI 生成事件（AiEvent）：`ai_generate_*` 后台任务推送，
 * phase = status | delta | done | cancelled | failed。
 */
export function onAiEvent(handler: (payload: AiEvent) => void): Promise<() => void> {
  return events.aiEvent.listen((e) => handler(e.payload));
}

/**
 * P7 凭据请求事件（CredentialPrompt）：CredentialBroker 经 UI 桥推送，
 * kind = https | askpass | host_key；前端弹三动作对话框后回 credential_respond。
 */
export function onCredentialRequest(
  handler: (payload: CredentialPrompt) => void
): Promise<() => void> {
  return events.credentialPrompt.listen((e) => handler(e.payload));
}
