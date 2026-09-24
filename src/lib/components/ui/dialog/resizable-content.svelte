<script lang="ts">
	/**
	 * 可自由调大小的 Dialog.Content：右缘/底缘/右下角握把拖拽（含方向键
	 * 微调、角部双击复位），左上角锚定，inline style 压过基类的居中
	 * transform 与 sm:max-w 限制。打开时按 savedWidth/savedHeight 恢复
	 * 尺寸并居中；主窗口缩小时钳回可视范围。
	 *
	 * 持久化完全交给调用方：拖拽/微调结束回调 onPersist(宽, 高)，
	 * 双击复位回调 onReset()（调用方可自行约定"恢复默认"的存法）。
	 */
	import { untrack } from "svelte";
	import { cn } from "$lib/utils.js";
	import * as Dialog from "./index.js";
	import type { WithoutChildrenOrChild } from "$lib/utils.js";
	import type { ComponentProps, Snippet } from "svelte";

	let {
		open,
		defaultWidth,
		defaultHeight,
		minWidth = 320,
		minHeight = 200,
		margin = 12, // 与主窗口边缘的最小间距
		savedWidth = 0,
		savedHeight = 0,
		onPersist,
		onReset,
		labels = {},
		class: className,
		children,
	}: {
		open: boolean;
		defaultWidth: number;
		defaultHeight: number;
		minWidth?: number;
		minHeight?: number;
		margin?: number;
		savedWidth?: number;
		savedHeight?: number;
		onPersist?: (width: number, height: number) => void;
		onReset?: () => void;
		labels?: { width?: string; height?: string; both?: string };
		class?: string;
		children?: Snippet;
	} & WithoutChildrenOrChild<ComponentProps<typeof Dialog.Content>> = $props();

	type Dir = "e" | "s" | "se";
	// 刻意只取初值：几何状态不随 props 变化（恢复尺寸走下方 $effect.pre）。
	// svelte-ignore state_referenced_locally
	let w = $state(defaultWidth);
	// svelte-ignore state_referenced_locally
	let h = $state(defaultHeight);
	let x = $state(0);
	let y = $state(0);
	let resizeDir = $state<Dir | null>(null);

	function clampSize(width: number, height: number): { width: number; height: number } {
		const maxW = Math.max(minWidth, window.innerWidth - margin * 2);
		const maxH = Math.max(minHeight, window.innerHeight - margin * 2);
		return {
			width: Math.min(maxW, Math.max(minWidth, Math.round(width))),
			height: Math.min(maxH, Math.max(minHeight, Math.round(height))),
		};
	}

	function center(width: number, height: number): void {
		x = Math.max(0, Math.round((window.innerWidth - width) / 2));
		y = Math.max(0, Math.round((window.innerHeight - height) / 2));
	}

	// 打开时恢复上次尺寸并居中；$effect.pre 渲染前执行，避免首帧位置闪跳。
	// untrack：拖拽提交的新尺寸不应触发重新居中。
	$effect.pre(() => {
		if (!open) return;
		untrack(() => {
			const c = clampSize(savedWidth || defaultWidth, savedHeight || defaultHeight);
			w = c.width;
			h = c.height;
			center(c.width, c.height);
		});
	});

	// 主窗口缩小时把弹窗钳回可视范围。
	$effect(() => {
		if (!open) return;
		const onViewportResize = (): void => {
			const c = clampSize(w, h);
			w = c.width;
			h = c.height;
			x = Math.min(x, Math.max(0, window.innerWidth - c.width));
			y = Math.min(y, Math.max(0, window.innerHeight - c.height));
		};
		window.addEventListener("resize", onViewportResize);
		return () => window.removeEventListener("resize", onViewportResize);
	});

	function beginResize(dir: Dir, event: PointerEvent): void {
		if (event.button !== 0) return;
		event.preventDefault();
		resizeDir = dir;
		const startX = event.clientX;
		const startY = event.clientY;
		const startW = w;
		const startH = h;
		(event.currentTarget as Element).setPointerCapture(event.pointerId);

		const onMove = (ev: PointerEvent): void => {
			const c = clampSize(
				dir === "s" ? startW : startW + (ev.clientX - startX),
				dir === "e" ? startH : startH + (ev.clientY - startY),
			);
			w = c.width;
			h = c.height;
		};
		const finish = (): void => {
			window.removeEventListener("pointermove", onMove);
			window.removeEventListener("pointerup", finish);
			window.removeEventListener("pointercancel", finish);
			resizeDir = null;
			onPersist?.(w, h);
		};
		window.addEventListener("pointermove", onMove);
		window.addEventListener("pointerup", finish);
		window.addEventListener("pointercancel", finish);
	}

	/** 握把聚焦后方向键微调，步进即持久化。 */
	function resizeKey(dir: Dir, event: KeyboardEvent): void {
		const dx = event.key === "ArrowLeft" ? -8 : event.key === "ArrowRight" ? 8 : 0;
		const dy = event.key === "ArrowUp" ? -8 : event.key === "ArrowDown" ? 8 : 0;
		if (dx === 0 && dy === 0) return;
		event.preventDefault();
		const c = clampSize(dir === "s" ? w : w + dx, dir === "e" ? h : h + dy);
		w = c.width;
		h = c.height;
		onPersist?.(c.width, c.height);
	}

	/** 双击角部握把：恢复默认尺寸并重新居中。 */
	function resetSize(): void {
		const c = clampSize(defaultWidth, defaultHeight);
		w = c.width;
		h = c.height;
		center(c.width, c.height);
		onReset?.();
	}
</script>

<Dialog.Content
	class={cn(resizeDir ? "select-none" : "", className)}
	style="left:{x}px; top:{y}px; width:{w}px; height:{h}px; max-width:none; translate:none;"
>
	{@render children?.()}

	<!-- 自由调大小握把：右缘 / 底缘 / 右下角（左上角锚定；角部双击复位） -->
	<button
		type="button"
		aria-label={labels.width ?? "Resize width"}
		class="absolute inset-y-0 right-0 z-10 w-1.5 cursor-ew-resize touch-none bg-transparent transition-colors outline-none hover:bg-primary/20 focus-visible:bg-primary/20 {resizeDir === 'e' ? 'bg-primary/30' : ''}"
		onpointerdown={(e) => beginResize("e", e)}
		onkeydown={(e) => resizeKey("e", e)}
	></button>
	<button
		type="button"
		aria-label={labels.height ?? "Resize height"}
		class="absolute inset-x-0 bottom-0 z-10 h-1.5 cursor-ns-resize touch-none bg-transparent transition-colors outline-none hover:bg-primary/20 focus-visible:bg-primary/20 {resizeDir === 's' ? 'bg-primary/30' : ''}"
		onpointerdown={(e) => beginResize("s", e)}
		onkeydown={(e) => resizeKey("s", e)}
	></button>
	<button
		type="button"
		aria-label={labels.both ?? "Reset size"}
		class="absolute right-0 bottom-0 z-20 size-4 cursor-nwse-resize touch-none bg-transparent transition-colors outline-none hover:bg-primary/20 focus-visible:bg-primary/20 {resizeDir === 'se' ? 'bg-primary/30' : ''}"
		onpointerdown={(e) => beginResize("se", e)}
		ondblclick={resetSize}
		onkeydown={(e) => resizeKey("se", e)}
	></button>
</Dialog.Content>
