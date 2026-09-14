import { writable, type Writable } from "svelte/store";

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

export function addToast(error: AppError): void {
  const toast: Toast = {
    id: nextId++,
    type: "error",
    message: error.message,
    detail: error.detail ?? undefined,
    duration: 5000,
  };

  toasts.update((list) => [...list, toast]);

  if (toast.duration && toast.duration > 0) {
    setTimeout(() => {
      removeToast(toast.id);
    }, toast.duration);
  }
}

export function removeToast(id: number): void {
  toasts.update((list) => list.filter((t) => t.id !== id));
}

export { toasts };
