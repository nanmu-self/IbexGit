<script lang="ts">
  // DiffViewer consumes DiffModel only (P4)
  export let diffModel: unknown = null;
</script>

<div class="diff-viewer">
  {#if diffModel}
    <pre>{JSON.stringify(diffModel, null, 2)}</pre>
  {:else}
    <p>No diff loaded</p>
  {/if}
</div>
