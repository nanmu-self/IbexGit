<script lang="ts">
  import "../app.css";
  import ToastHost from "$lib/components/ui/toast/ToastHost.svelte";
  import CommandPalette from "$lib/components/palette/CommandPalette.svelte";
  import SettingsDialog from "$lib/components/settings/SettingsDialog.svelte";
import RepoSettingsDialog from "$lib/components/settings/RepoSettingsDialog.svelte";
  import { settings } from "$lib/stores/settings.svelte";
import { appDialogs } from "$lib/stores/appdialogs.svelte";
  import { repos } from "$lib/stores/repos.svelte";
  import { initKeyboard } from "$lib/keyboard";
  import { t } from "$lib/i18n";
  import { getCurrentWebview } from "@tauri-apps/api/webview";

  let { children } = $props();
  let dragOver = $state(false);

  // Settings store init (idempotent) — gates session restore in +page.svelte.
  $effect(() => {
    void settings.init();
  });

  // Global keyboard dispatcher (keymap single source, PLAN §4.9).
  $effect(() => initKeyboard());

  // Theme: resolve 'system' against the OS preference.
  $effect(() => {
    const mode = settings.theme;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const dark = mode === "dark" || (mode === "system" && mq.matches);
      document.documentElement.classList.toggle("dark", dark);
    };
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  });

  // P10 设置中心：可配置的编辑器字体与 tab 宽度（diff 正文 / 冲突编辑器）。
  $effect(() => {
    const root = document.documentElement;
    const font = settings.editorFont.trim();
    if (font) {
      root.style.setProperty(
        "--font-editor",
        `'${font.replace(/'/g, "")}', var(--font-mono, ui-monospace)`,
      );
    } else {
      root.style.removeProperty("--font-editor");
    }
    root.style.setProperty("--editor-tab-size", String(settings.editorTabSize));
  });

  // Backend events (watcher refresh + single-instance open) and window
  // drag-drop (Tauri intercepts drops; paths arrive via these events).
  $effect(() => {
    const offEvents = repos.initEvents();
    let unlistenDrag: (() => void) | undefined;
    let cancelled = false;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          dragOver = true;
        } else if (payload.type === "leave") {
          dragOver = false;
        } else if (payload.type === "drop") {
          dragOver = false;
          for (const path of payload.paths) void repos.openPath(path);
        }
      })
      .then((un) => {
        if (cancelled) un();
        else unlistenDrag = un;
      });
    return () => {
      cancelled = true;
      offEvents();
      unlistenDrag?.();
    };
  });
</script>

{@render children()}

{#if dragOver}
  <div
    class="pointer-events-none fixed inset-0 z-50 flex items-center justify-center bg-primary/10 backdrop-blur-xs"
  >
    <div
      class="rounded-xl border-2 border-dashed border-primary/60 bg-background/95 px-8 py-6 text-sm font-medium shadow-lg"
    >
      {t("app.dropToOpen")}
    </div>
  </div>
{/if}

<ToastHost />
<CommandPalette />
<SettingsDialog />
<RepoSettingsDialog
  bind:open={appDialogs.repoSettingsOpen}
  repoId={appDialogs.repoSettingsRepoId}
/>
