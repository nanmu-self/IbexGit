<script lang="ts">
  // Minimal auto-growing textarea for the tag message (no bits-ui textarea
  // component in the project; styled to match `Input`).
  let {
    value = $bindable(""),
    placeholder = "",
    rows = 3,
  }: { value?: string; placeholder?: string; rows?: number } = $props();
</script>

<textarea
  bind:value
  {placeholder}
  rows={rows}
  class="flex min-h-16 w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-xs placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:border-ring disabled:cursor-not-allowed disabled:opacity-50"
></textarea>
