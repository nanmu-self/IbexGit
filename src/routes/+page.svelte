<script lang="ts">
  import { onMount } from "svelte";
  import { commands, normalizeError } from "$lib/git";
  import { toasts, removeToast } from "$lib/stores/toast";

  let gitCapabilities = $state<string>("Loading...");
  let errorMessage = $state<string>("");
  let isDark = $state(false);

  onMount(async () => {
    try {
      const caps = await commands.gitVersion();
      gitCapabilities = caps;
    } catch (e) {
      errorMessage = e instanceof Error ? e.message : String(e);
    }
  });

  function toggleTheme() {
    isDark = !isDark;
    document.documentElement.classList.toggle("dark", isDark);
  }
</script>

<main class="flex flex-col h-screen">
  <!-- Top Bar -->
  <header class="flex items-center justify-between px-4 py-2 border-b border-gray-200 dark:border-gray-800 bg-white dark:bg-gray-900">
    <div class="flex items-center gap-2">
      <div class="w-8 h-8 rounded bg-blue-600 flex items-center justify-center text-white font-bold">I</div>
      <span class="font-semibold text-gray-900 dark:text-gray-100">IbexGit</span>
    </div>
    <div class="flex items-center gap-2">
      <button
        class="px-3 py-1.5 text-sm rounded-md border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 text-gray-700 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-gray-700"
        onclick={toggleTheme}
      >
        {isDark ? "Light" : "Dark"}
      </button>
    </div>
  </header>

  <!-- Content -->
  <div class="flex-1 flex items-center justify-center p-8 bg-gray-50 dark:bg-gray-950">
    <div class="max-w-2xl w-full space-y-6">
      <div class="text-center space-y-2">
        <h1 class="text-3xl font-bold text-gray-900 dark:text-gray-100">IbexGit</h1>
        <p class="text-gray-600 dark:text-gray-400">Local-first Git client · P0 空壳验证</p>
      </div>

      <!-- Git Version Card -->
      <div class="rounded-lg border border-gray-200 dark:border-gray-800 bg-white dark:bg-gray-900 p-4 shadow-sm">
        <h2 class="text-sm font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider mb-2">
          Git Capabilities
        </h2>
        {#if errorMessage}
          <div class="text-red-600 dark:text-red-400 text-sm">
            Failed to load: {errorMessage}
          </div>
        {:else}
          <pre class="text-xs text-gray-800 dark:text-gray-200 whitespace-pre-wrap">{gitCapabilities}</pre>
        {/if}
      </div>

      <!-- Actions -->
      <div class="flex flex-wrap gap-3">
        <button
          class="px-4 py-2 rounded-md bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
          onclick={async () => {
            try {
              await commands.greet("P0");
            } catch (e) {
              // surfaced as toast
              normalizeError(e);
            }
          }}
        >
          Test Greet
        </button>
        <button
          class="px-4 py-2 rounded-md bg-red-600 text-white hover:bg-red-700"
          onclick={async () => {
            try {
              await commands.gitStatus("not-a-repo-id");
            } catch (e) {
              // surfaced as toast
              normalizeError(e);
            }
          }}
        >
          Test Error Toast
        </button>
      </div>
    </div>
  </div>

  <!-- Status Bar -->
  <footer class="flex items-center justify-between px-4 py-1.5 border-t border-gray-200 dark:border-gray-800 bg-white dark:bg-gray-900 text-xs text-gray-500 dark:text-gray-400">
    <span>P0 · Architecture bootstrap</span>
    <span>No repository open</span>
  </footer>
</main>

<!-- Toast Container -->
<div class="fixed bottom-4 right-4 z-50 flex flex-col gap-2">
  {#each $toasts as toast (toast.id)}
    <div
      class="flex items-start gap-3 rounded-lg border border-gray-200 dark:border-gray-800 bg-white dark:bg-gray-900 p-3 shadow-lg min-w-[300px] max-w-md"
      role="alert"
    >
      <div class="flex-1">
        <div class="text-sm font-medium text-gray-900 dark:text-gray-100">
          {toast.type === "error" ? "Error" : toast.type === "success" ? "Success" : toast.type}
        </div>
        {#if toast.detail}
          <div class="text-xs text-gray-500 dark:text-gray-400 mt-0.5">{toast.detail}</div>
        {/if}
      </div>
      <button
        class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
        onclick={() => removeToast(toast.id)}
      >
        ×
      </button>
    </div>
  {/each}
</div>

<style>
  /* Dark mode is toggled via class on <html> */
  :global(html.dark) {
    color-scheme: dark;
  }
  :global(html:not(.dark)) {
    color-scheme: light;
  }
</style>
