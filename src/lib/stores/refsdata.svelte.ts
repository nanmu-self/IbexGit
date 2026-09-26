/**
 * Shared refs data (P6): tags / remotes / stashes / reflog of the active
 * repository. Loaded on repo switch, watcher refresh and after any refs
 * mutation; rendered by the sidebar RefPanel, TagsView and dialog hosts.
 */
import { git, normalizeError } from "$lib/git";
import type { BranchInfo, ReflogEntry, RemoteInfo, StashEntry, TagInfo } from "$lib/git/bindings";

export const REFS_RELOG_LIMIT = 100;

export const refsData = $state<{
  repoId: string | null;
  loading: boolean;
  tags: TagInfo[];
  remotes: RemoteInfo[];
  /** Remote-tracking branches of all remotes (origin/…). */
  remoteBranches: BranchInfo[];
  stashes: StashEntry[];
  /** HEAD reflog (browser section). */
  reflog: ReflogEntry[];
  /** Which ref the reflog view is showing ("HEAD" by default). */
  reflogRef: string;
}>({
  repoId: null,
  loading: false,
  tags: [],
  remotes: [],
  remoteBranches: [],
  stashes: [],
  reflog: [],
  reflogRef: "HEAD",
});

/** Reload everything (reflog only when the section is showing HEAD moves). */
export async function loadRefsData(id: string | null, reflogRef?: string): Promise<void> {
  if (!id) {
    refsData.repoId = null;
    refsData.tags = [];
    refsData.remotes = [];
    refsData.remoteBranches = [];
    refsData.stashes = [];
    refsData.reflog = [];
    return;
  }
  if (reflogRef !== undefined && reflogRef !== refsData.reflogRef) {
    refsData.reflogRef = reflogRef;
    refsData.reflog = [];
  }
  refsData.repoId = id;
  refsData.loading = true;
  try {
    const ref = refsData.reflogRef;
    const [tags, remotes, remoteBranches, stashes, reflog] = await Promise.all([
      git.tags(id),
      git.remotes(id),
      git.remoteBranches(id),
      git.stashList(id),
      git.reflog(id, ref),
    ]);
    if (refsData.repoId !== id) return; // switched away meanwhile
    refsData.tags = tags;
    refsData.remotes = remotes;
    refsData.remoteBranches = remoteBranches;
    refsData.stashes = stashes;
    refsData.reflog = reflog;
  } catch (e) {
    normalizeError(e);
  } finally {
    refsData.loading = false;
  }
}
