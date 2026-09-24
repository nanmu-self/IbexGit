import type { Component } from "svelte";

/** Tree node model (PLAN §5 基础组件库：Tree)。 */
export interface TreeNode {
  id: string;
  label: string;
  icon?: Component<{ class?: string }>;
  /** Right-aligned pill (counts). */
  badge?: string | number | null;
  /** Pill tone: `red` accent (conflict counts) or neutral. */
  badgeTone?: "red" | "muted";
  /** Right-aligned plain text (e.g. ahead/behind). */
  trailing?: string;
  /** Highlight as currently selected (e.g. checked-out branch). */
  current?: boolean;
  /** Non-interactive placeholder leaf. */
  muted?: boolean;
  /** Free-form payload (view id, branch name…) interpreted by the caller. */
  payload?: string;
  children?: TreeNode[];
}

export interface TreeProps {
  nodes: TreeNode[];
  /** Fully controlled expanded group ids. */
  expanded: Set<string>;
  activeId?: string | null;
  onToggle?: (node: TreeNode, expanded: boolean) => void;
  /** `e` lets callers implement ctrl/shift-click multi-select. */
  onActivate?: (node: TreeNode, e?: MouseEvent) => void;
  onActivateSecondary?: (node: TreeNode) => void;
  /** Right-click on a row (context menus); absent → browser menu. */
  onLeafContext?: (node: TreeNode, e: MouseEvent) => void;
  depth?: number;
}
