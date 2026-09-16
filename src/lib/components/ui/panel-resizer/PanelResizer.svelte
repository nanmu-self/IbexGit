<script lang="ts">
  let {
    width = $bindable(248),
    min = 180,
    max = 480,
    /** Which side of the grip the sized panel sits on: "left" → dragging
     *  right widens it (sidebar, file list); "right" → dragging right
     *  narrows it (history detail panel). */
    side = "left",
    /** True while pointer-dragging; width consumers use it to drop their
     *  width transition so the panel tracks the cursor 1:1 (keyboard steps
     *  keep the transition and glide). */
    dragging = $bindable(false),
    /** Called with the final width when a drag ends or after a keyboard
     *  step — call sites persist it here (not on every pointermove). */
    onCommit = undefined,
  }: {
    width?: number;
    min?: number;
    max?: number;
    side?: "left" | "right";
    dragging?: boolean;
    onCommit?: (width: number) => void;
  } = $props();

  // clientX deltas (not movementX): reliable across engines/WebView versions.
  let lastX = 0;
  /** Whether the current drag actually changed the width. */
  let moved = false;

  function onPointerDown(event: PointerEvent): void {
    dragging = true;
    moved = false;
    lastX = event.clientX;
    (event.currentTarget as Element).setPointerCapture(event.pointerId);
  }

  function onPointerMove(event: PointerEvent): void {
    if (!dragging) return;
    const delta = event.clientX - lastX;
    lastX = event.clientX;
    if (delta === 0) return;
    moved = true;
    width = Math.min(max, Math.max(min, width + (side === "left" ? delta : -delta)));
  }

  function onPointerUp(): void {
    if (!dragging) return;
    dragging = false;
    if (moved) onCommit?.(width);
  }

  function onKeyDown(event: KeyboardEvent): void {
    // Arrow keys move the grip visually; panel width follows accordingly.
    const step = event.key === "ArrowLeft" ? -8 : event.key === "ArrowRight" ? 8 : 0;
    if (step === 0) return;
    event.preventDefault();
    width = Math.min(max, Math.max(min, width + (side === "left" ? step : -step)));
    onCommit?.(width);
  }
</script>

<!-- Must sit directly in a flex row (align-self stretch keeps it full-height):
     wrapping it in a plain div collapses the grip to height 0. The ::before
     widens the hit area to ~12px while the visible line stays 4px. -->
<button
  type="button"
  aria-label="Resize panel"
  class="relative w-1 shrink-0 cursor-col-resize touch-none self-stretch bg-transparent transition-colors before:absolute before:inset-y-0 before:-left-1.5 before:-right-1.5 before:content-[''] hover:bg-primary/20 {dragging
    ? 'bg-primary/30'
    : ''}"
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={onPointerUp}
  onkeydown={onKeyDown}
></button>
