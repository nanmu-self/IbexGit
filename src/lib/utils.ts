import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";
import type { Component, Snippet } from "svelte";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

// shadcn-svelte v1.6 component helper types (expected from $lib/utils).
export type WithoutChild<T> = T extends { child?: unknown } ? Omit<T, "child"> : T;
export type WithoutChildren<T> = T extends { children?: unknown } ? Omit<T, "children"> : T;
export type WithoutChildrenOrChild<T> = WithoutChildren<WithoutChild<T>>;
export type WithElementRef<T, U extends HTMLElement = HTMLElement> = T & { ref?: U | null };
export type WithChildren<T> = T & { children?: Snippet; components?: Component };
