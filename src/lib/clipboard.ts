/**
 * 剪贴板小工具：复制文本 / 远程仓库地址并给出 toast 反馈。
 *
 * 桌面端统一走 Tauri clipboard-manager 插件（navigator.clipboard 在部分
 * webview 下不可靠）。远程地址的"一步复制"入口（标签页右键 / 命令面板）
 * 与逐 remote 复制（侧栏远程区 / 仓库设置弹窗）共用此处。
 */
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { t } from "$lib/i18n";
import { showToast } from "$lib/stores/toast";
import { git, normalizeError, type RepoId, type RemoteInfo } from "$lib/git";

/** 复制文本并提示成功；失败静默（返回 false，调用方可自行兜底）。 */
export async function copyText(text: string, toastKey: string): Promise<boolean> {
  try {
    await writeText(text);
    showToast("success", t(toastKey));
    return true;
  } catch {
    return false;
  }
}

/** 远程的规范地址（url 缺省时回退 fetch / push URL）。 */
export function remoteUrl(r: RemoteInfo): string {
  return r.url || r.fetch_url || r.push_url;
}

/** 主远程：优先 origin，否则第一个 remote。 */
function primaryRemote(remotes: RemoteInfo[]): RemoteInfo | null {
  return remotes.find((r) => r.name === "origin") ?? remotes[0] ?? null;
}

/** 复制仓库地址（主远程的 URL）；未配置远程时提示。 */
export async function copyPrimaryRemoteUrl(repoId: RepoId): Promise<void> {
  let remotes: RemoteInfo[];
  try {
    remotes = await git.remotes(repoId);
  } catch (err) {
    normalizeError(err);
    return;
  }
  const r = primaryRemote(remotes);
  if (!r) {
    showToast("info", t("clipboard.noRemote"));
    return;
  }
  await copyText(remoteUrl(r), "clipboard.remoteUrlCopied");
}
