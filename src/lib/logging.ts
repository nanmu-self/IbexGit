/**
 * 前端错误捕获 → Rust 文件日志（`app_log` 命令，target `frontend`），
 * 与 Rust 侧 tracing 落进同一份 `{appData}/logs/` 文件，方便用户
 * 反馈问题时拿到完整现场。
 *
 * 捕获来源：window error / unhandledrejection / console.error / console.warn。
 * 原始 console 行为保持不变（devtools 照常输出），只额外转发。
 *
 * 安全阀（日志通道绝不能成为新的故障源）：
 * - 转发失败静默吞掉（否则 invoke 失败可能再次触发错误钩子 → 递归）；
 * - 相同消息 2s 内去重；10s 窗口内最多 30 条，超限丢弃；
 * - 单条消息截断到 4000 字符（Rust 侧再兜底一次）。
 */
import { commands, type FrontendLogLevel } from "$lib/git/bindings";

const MAX_CHARS = 4000;
const DEDUP_MS = 2000;
const BURST_LIMIT = 30;
const BURST_WINDOW_MS = 10_000;

let installed = false;
let lastMessage = "";
let lastSentAt = 0;
let burstCount = 0;
let burstStart = 0;

function clip(message: string): string {
  return message.length > MAX_CHARS ? message.slice(0, MAX_CHARS) + "…" : message;
}

function send(level: FrontendLogLevel, message: string): void {
  const now = Date.now();
  if (message === lastMessage && now - lastSentAt < DEDUP_MS) return;
  if (now - burstStart >= BURST_WINDOW_MS) {
    burstStart = now;
    burstCount = 0;
  }
  if (burstCount >= BURST_LIMIT) return;
  burstCount++;
  lastMessage = message;
  lastSentAt = now;
  // catch 必须为空：这条链路不允许产生新的 rejection。
  commands.appLog(level, clip(message)).catch(() => {});
}

function describe(value: unknown): string {
  if (value instanceof Error) return value.stack || `${value.name}: ${value.message}`;
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

/** 安装全局钩子（幂等）。必须在客户端环境调用（+layout 的 $effect 内）。 */
export function installFrontendLogging(): void {
  if (installed) return;
  installed = true;

  window.addEventListener("error", (event) => {
    // 资源加载错误（img/script 404 等）没有 error 对象，走 message+位置；
    // JS 异常优先带 stack。
    const detail =
      event.error instanceof Error
        ? `${event.message}\n${event.error.stack ?? ""}`
        : `${event.message} @ ${event.filename}:${event.lineno}:${event.colno}`;
    send("error", detail);
  });

  window.addEventListener("unhandledrejection", (event) => {
    send("error", `unhandled rejection: ${describe(event.reason)}`);
  });

  const origError = console.error.bind(console);
  console.error = (...args: unknown[]) => {
    send("error", args.map(describe).join(" "));
    origError(...args);
  };
  const origWarn = console.warn.bind(console);
  console.warn = (...args: unknown[]) => {
    send("warn", args.map(describe).join(" "));
    origWarn(...args);
  };
}
