import { writable, type Writable } from "svelte/store";

/** Normalized error shape (serde `tag = "code"` on the Rust side). */
export interface AppError {
  code: string;
  message: string;
  detail: string | null;
}

export interface Toast {
  id: number;
  type: "error" | "success" | "info" | "warning";
  message: string;
  detail?: string;
  duration?: number;
}

const toasts: Writable<Toast[]> = writable([]);

let nextId = 1;

/** Generic toast entry (P2: also used for info/success notifications). */
export function showToast(
  type: Toast["type"],
  message: string,
  detail?: string,
  duration = 3500,
): void {
  const toast: Toast = { id: nextId++, type, message, detail, duration };
  toasts.update((list) => [...list, toast]);
  if (toast.duration && toast.duration > 0) {
    setTimeout(() => removeToast(toast.id), toast.duration);
  }
}

/** Normalize pipeline entry point: errors surface as error toasts. */
export function addToast(error: AppError): void {
  showToast("error", error.message, error.detail ?? undefined, 5000);
}

export function removeToast(id: number): void {
  toasts.update((list) => list.filter((t) => t.id !== id));
}

export { toasts };
