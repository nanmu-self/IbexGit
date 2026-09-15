<script lang="ts">
  let {
    width = $bindable(248),
    min = 180,
    max = 480,
    /** True while pointer-dragging; width consumers use it to drop their
     *  width transition so the panel tracks the cursor 1:1 (keyboard steps
     *  keep the transition and glide). */
    dragging = $bindable(false),
  }: { width?: number; min?: number; max?: number; dragging?: boolean } = $props();

  function onPointerDown(event: PointerEvent): void {
    dragging = true;
    (event.currentTarget as Element).setPointerCapture(event.pointerId);
  }

  function onPointerMove(event: PointerEvent): void {
    if (!dragging) return;
    width = Math.min(max, Math.max(min, width + event.movementX));
  }

  function onPointerUp(): void {
    dragging = false;
  }

  function onKeyDown(event: KeyboardEvent): void {
    if (event.key === "ArrowLeft") {
      width = Math.max(min, width - 8);
      event.preventDefault();
    } else if (event.key === "ArrowRight") {
      width = Math.min(max, width + 8);
      event.preventDefault();
    }
  }
</script>

<button
  type="button"
  aria-label="Resize panel"
  class="w-1 shrink-0 cursor-col-resize bg-transparent transition-colors hover:bg-primary/20 {dragging
    ? 'bg-primary/30'
    : ''}"
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={onPointerUp}
  onkeydown={onKeyDown}
></button>
