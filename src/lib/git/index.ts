import { invoke } from "@tauri-apps/api/core";
import { addToast } from "$lib/stores/toast";
import type { AppError } from "$lib/stores/toast";

/**
 * Unified invoke wrapper that converts Rust errors into typed AppError.
 */
export async function gitInvoke<T>(
  command: string,
  args?: Record<string, unknown>
): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (raw) {
    // Tauri wraps serialized errors as objects; normalize them.
    const err = raw as { code?: string; message?: string; detail?: string } | string;
    let appError: AppError;

    if (typeof err === "string") {
      appError = {
        code: "internal",
        message: err,
        detail: null,
      };
    } else {
      appError = {
        code: (err as any).code || "internal",
        message: (err as any).message || "Unknown error",
        detail: (err as any).detail || null,
      };
    }

    addToast(appError);
    throw appError;
  }
}
