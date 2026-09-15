import { open } from "@tauri-apps/plugin-dialog";
import { repos } from "$lib/stores/repos.svelte";

/** Native folder picker → repos.openPath (shared by menu/toolbar/tabs/welcome). */
export async function pickRepo(): Promise<void> {
  const path = await open({ directory: true, multiple: false });
  if (typeof path === "string" && path.length > 0) {
    await repos.openPath(path);
  }
}
