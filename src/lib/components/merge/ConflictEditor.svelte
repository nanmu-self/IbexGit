<script lang="ts">
  // Conflict editor consumes ConflictModel only (P8)
  export let conflict: unknown = null;
</script>

<div class="conflict-editor">
  {#if conflict}
    <pre>{JSON.stringify(conflict, null, 2)}</pre>
  {:else}
    <p>No conflict</p>
  {/if}
</div>
