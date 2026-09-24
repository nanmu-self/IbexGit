<script lang="ts">
/**
 * SegmentedControl —— 等宽分段选择器，radio group 语义。
 * 替代 SettingsDialog 里重复的 `{#each items as x} <button class="rounded-md border ...">` 样板。
 *
 * 两种用法：
 * <SegmentedControl bind:value={mode} options={[...]} /> —— 直接赋值
 * <SegmentedControl value={mode} options={[...]} onselect={(v) => save(v)} /> —— 走 setter/持久化
 */
export interface SegmentedOption {
	value: string;
	label: string;
}

let {
	value = $bindable<string>(),
	options = [] as SegmentedOption[],
	onselect,
	itemClass = "",
	class: className = "",
}: {
	value?: string;
	options?: SegmentedOption[];
	onselect?: (value: string) => void;
	itemClass?: string;
	class?: string;
} = $props();

function select(next: string): void {
	if (onselect) {
		onselect(next);
	} else {
		value = next;
	}
}
</script>

<div
	class={`flex gap-1.5 ${className}`}
	role="radiogroup"
>
	{#each options as opt (opt.value)}
		<button
			type="button"
			role="radio"
			aria-checked={value === opt.value}
			class="flex-1 rounded-md border px-2 py-1.5 text-micro transition-colors {itemClass} {value === opt.value
				? 'border-primary bg-primary/10 text-foreground'
				: 'text-muted-foreground hover:bg-accent/50'}"
			onclick={() => select(opt.value)}
		>
			{opt.label}
		</button>
	{/each}
</div>
