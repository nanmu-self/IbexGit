<script lang="ts">
  // CommitGraph consumes layout results only (P5)
  export let commits: unknown[] = [];
</script>

<div class="commit-graph">
  <p>{commits.length} commits</p>
</div>
