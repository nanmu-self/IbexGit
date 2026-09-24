<script lang="ts">
  /**
   * RefsDialogsHost (P6) — global owner of every refs dialog. Mounted once
   * in +page.svelte; entry points (sidebar, toolbar, history, tags view)
   * fire `RefAction`s through the bus and this host executes them with
   * toasts, undo anchors and refreshes (repos + refsdata).
   */
  import ConfirmDialog from "$lib/components/ui/confirm-dialog/ConfirmDialog.svelte";
  import InputDialog from "./dialogs/InputDialog.svelte";
  import NewTagDialog from "./dialogs/NewTagDialog.svelte";
  import ResetDialog from "./dialogs/ResetDialog.svelte";
  import CleanDialog from "./dialogs/CleanDialog.svelte";
  import CompareDialog from "./dialogs/CompareDialog.svelte";
  import PushDialog from "./dialogs/PushDialog.svelte";
  import PullDialog from "./dialogs/PullDialog.svelte";
  import MergeRebaseDialog from "./dialogs/MergeRebaseDialog.svelte";
  import StashDiffDialog from "./dialogs/StashDiffDialog.svelte";
  import BackupListDialog from "./dialogs/BackupListDialog.svelte";
  import RemoteDialog from "./dialogs/RemoteDialog.svelte";
  import { t } from "$lib/i18n";
  import { repos } from "$lib/stores/repos.svelte";
  import { refsData, loadRefsData } from "$lib/stores/refsdata.svelte";
  import { onRefAction, type RefAction } from "$lib/stores/refbus";
  import { conflictEntered, runFetch, runPull, runPush } from "$lib/stores/netops.svelte";
  import { showToast } from "$lib/stores/toast";
  import { git, recovery, normalizeError, type StashEntry } from "$lib/git";
  import type { BackupRef } from "$lib/git/bindings";

  const active = $derived(repos.active);
  const repoId = $derived(repos.activeId);

  async function refreshAll(): Promise<void> {
    const id = repoId;
    if (id !== null) {
      await repos.refresh(id);
      await loadRefsData(id);
    }
  }

  // ---- bus wiring ----
  $effect(() => {
    return onRefAction((a) => void handle(a));
  });

  async function handle(a: RefAction): Promise<void> {
    if (a.kind === "fetch") {
      if (repoId !== null) await runFetch(repoId, a.remote ?? null);
      return;
    }
    if (a.kind === "pull") {
      if (repoId !== null) {
        pullOpen = true;
      }
      return;
    }
    if (a.kind === "push") {
      if (repoId !== null) pushOpen = true;
      return;
    }
    if (a.kind === "stashView") {
      stashDiffEntry = refsData.stashes.find((s) => s.index === a.index) ?? null;
      stashDiffOpen = stashDiffEntry !== null;
      return;
    }
    if (a.kind === "stashDrop") {
      stashDropIndex = a.index;
      stashDropOpen = true;
      return;
    }
    if (a.kind === "reflogRestore") {
      askReflogRestore(a.hash, a.subject);
      return;
    }
    switch (a.kind) {
      case "newBranch":
        newBranchValue = "";
        newBranchStart = a.start ?? "";
        newBranchOpen = true;
        break;
      case "renameBranch":
        renameTarget = a.name;
        renameValue = a.name;
        renameOpen = true;
        break;
      case "setUpstream":
        upstreamTarget = a.branch;
        upstreamValue = a.upstream ?? "";
        upstreamOpen = true;
        break;
      case "deleteBranch":
        await askDeleteBranch(a.name);
        break;
      case "reset":
        resetInitialTarget = a.initialTarget ?? "";
        resetOpen = true;
        break;
      case "clean":
        await openClean();
        break;
      case "backups":
        await openBackups();
        break;
      case "newTag":
        tagTarget = a.target ?? "HEAD";
        tagOpen = true;
        break;
      case "deleteTag":
        deleteTagTarget = a.name;
        deleteTagOpen = true;
        break;
      case "addRemote":
        remoteEditName = "";
        remoteEditUrl = "";
        remoteOpen = true;
        break;
      case "editRemote": {
        const r = refsData.remotes.find((x) => x.name === a.name);
        remoteEditName = a.name;
        remoteEditUrl = r?.fetch_url ?? "";
        remoteOpen = true;
        break;
      }
      case "removeRemote":
        removeRemoteTarget = a.name;
        removeRemoteOpen = true;
        break;
      case "merge":
        mergeMode = "merge";
        mergeInitialTarget = a.target;
        mergeOpen = true;
        break;
      case "rebase":
        mergeMode = "rebase";
        mergeInitialTarget = a.target;
        mergeOpen = true;
        break;
      case "compare":
        compareLeft = a.left;
        compareRight = a.right ?? repos.active?.branch ?? "";
        compareOpen = true;
        break;
    }
  }

  // ---- compare dialog state ----
  let compareOpen = $state(false);
  let compareLeft = $state("");
  let compareRight = $state("");

  // ===================== branch =====================
  let newBranchOpen = $state(false);
  let newBranchValue = $state("");
  /** Optional start point (branch/tag/commit the new branch should point at). */
  let newBranchStart = $state("");
  let branchBusy = $state(false);

  async function submitNewBranch(name: string): Promise<void> {
    if (repoId === null) return;
    branchBusy = true;
    try {
      await git.createBranch(repoId, name, newBranchStart || undefined);
      newBranchOpen = false;
      showToast("success", t("refs.branchCreated", { name }));
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    } finally {
      branchBusy = false;
    }
  }

  let renameOpen = $state(false);
  let renameTarget = $state("");
  let renameValue = $state("");

  async function submitRename(name: string): Promise<void> {
    if (repoId === null) return;
    branchBusy = true;
    try {
      await git.renameBranch(repoId, renameTarget, name);
      renameOpen = false;
      showToast("success", t("refs.branchRenamed", { old: renameTarget, name }));
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    } finally {
      branchBusy = false;
    }
  }

  let upstreamOpen = $state(false);
  let upstreamTarget = $state("");
  let upstreamValue = $state("");

  async function submitUpstream(value: string): Promise<void> {
    if (repoId === null) return;
    branchBusy = true;
    try {
      await git.setUpstream(repoId, upstreamTarget, value || null);
      upstreamOpen = false;
      showToast("success", t("refs.upstreamSet", { branch: upstreamTarget }));
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    } finally {
      branchBusy = false;
    }
  }

  // Delete: try the safe `-d` first; when git refuses ("not fully merged")
  // offer the destructive force-delete with an explicit warning.
  let deleteBranchTarget = $state<string | null>(null);
  let forceDelete = $state(false);
  let deleteBranchOpen = $state(false);

  async function askDeleteBranch(name: string): Promise<void> {
    if (repoId === null) return;
    deleteBranchTarget = name;
    forceDelete = false;
    try {
      await git.deleteBranch(repoId, name, false);
      showToast("success", t("refs.branchDeleted", { name }));
      await refreshAll();
    } catch (e) {
      const err = e as { message?: string };
      if (/not fully merged|not merged/i.test(err.message ?? "")) {
        forceDelete = true;
        deleteBranchOpen = true;
      } else {
        normalizeError(e);
      }
    }
  }

  async function confirmDeleteBranch(): Promise<void> {
    const name = deleteBranchTarget;
    if (repoId === null || !name) return;
    branchBusy = true;
    try {
      await git.deleteBranch(repoId, name, forceDelete);
      showToast("success", t("refs.branchDeleted", { name }));
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    } finally {
      branchBusy = false;
    }
  }

  // ===================== reset / clean / backups =====================
  let resetOpen = $state(false);
  /** Prefilled target (e.g. opened as 撤销提交 from the history context menu). */
  let resetInitialTarget = $state("");
  let resetBusy = $state(false);
  /** Undo anchors of the most recent reset (one-click undo). */
  let lastReset = $state<{ backupRef: string; mode: string; snapshotId: string | null } | null>(
    null
  );

  async function executeReset(mode: string, target: string): Promise<void> {
    if (repoId === null) return;
    resetBusy = true;
    try {
      const undo = await git.reset(repoId, mode, target);
      lastReset = {
        backupRef: undo.backup_ref,
        mode,
        snapshotId: undo.snapshot_id,
      };
      showToast(
        "success",
        t("refs.reset.done", { mode, target }),
        t("refs.reset.undoHint"),
        10000,
        {
          label: t("refs.reset.undo"),
          run: () => void undoReset(),
        }
      );
      await refreshAll();
    } finally {
      resetBusy = false;
    }
  }

  async function undoReset(): Promise<void> {
    const u = lastReset;
    if (repoId === null || !u) return;
    try {
      await git.undoReset(repoId, u.backupRef, u.mode, u.snapshotId);
      lastReset = null;
      showToast("success", t("refs.reset.undone"));
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    }
  }

  let cleanOpen = $state(false);
  let cleanPaths = $state<string[]>([]);
  let cleanLoading = $state(false);
  let cleanBusy = $state(false);

  async function openClean(): Promise<void> {
    if (repoId === null) return;
    cleanOpen = true;
    cleanLoading = true;
    try {
      cleanPaths = await git.cleanList(repoId);
    } catch (e) {
      normalizeError(e);
      cleanOpen = false;
    } finally {
      cleanLoading = false;
    }
  }

  async function confirmClean(selected: string[]): Promise<void> {
    if (repoId === null) return;
    cleanBusy = true;
    try {
      const snapId = await git.clean(repoId, selected);
      showToast(
        "success",
        t("refs.clean.done", { n: selected.length }),
        t("workspace.discardUndoHint"),
        8000,
        snapId
          ? {
              label: t("workspace.undo"),
              run: () => void restoreSnapshot(snapId),
            }
          : undefined
      );
      await refreshAll();
    } finally {
      cleanBusy = false;
    }
  }

  async function restoreSnapshot(snapshotId: string): Promise<void> {
    if (repoId === null) return;
    try {
      await recovery.restore(repoId, snapshotId);
      await refreshAll();
      showToast("success", t("workspace.restored"));
    } catch (e) {
      normalizeError(e);
    }
  }

  let backupsOpen = $state(false);
  let backupsLoading = $state(false);
  let backupsBusy = $state(false);

  async function openBackups(): Promise<void> {
    if (repoId === null) return;
    backupsOpen = true;
    backupsLoading = true;
    try {
      // The list itself is always fresh (not cached in refsData).
      backupRefs = await git.backupList(repoId);
    } catch (e) {
      normalizeError(e);
    } finally {
      backupsLoading = false;
    }
  }

  let backupRefs = $state<BackupRef[]>([]);

  async function deleteBackups(names: string[]): Promise<void> {
    if (repoId === null) return;
    backupsBusy = true;
    try {
      await git.backupDelete(repoId, names);
      backupRefs = backupRefs.filter((r) => !names.includes(r.full_name));
      showToast("success", t("refs.backups.deleted", { n: names.length }));
    } catch (e) {
      normalizeError(e);
    } finally {
      backupsBusy = false;
    }
  }

  // ===================== tags =====================
  let tagOpen = $state(false);
  let tagTarget = $state("HEAD");
  let tagBusy = $state(false);

  async function createTag(name: string, message: string | null, target: string): Promise<void> {
    if (repoId === null) return;
    tagBusy = true;
    try {
      await git.createTag(repoId, name, message, target);
      tagOpen = false;
      showToast("success", t("refs.tagCreated", { name }));
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    } finally {
      tagBusy = false;
    }
  }

  let deleteTagOpen = $state(false);
  let deleteTagTarget = $state("");

  async function confirmDeleteTag(): Promise<void> {
    if (repoId === null) return;
    try {
      await git.deleteTag(repoId, deleteTagTarget);
      showToast("success", t("refs.tagDeleted", { name: deleteTagTarget }));
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    }
  }

  // ===================== remotes =====================
  let remoteOpen = $state(false);
  let remoteEditName = $state("");
  let remoteEditUrl = $state("");
  let remoteBusy = $state(false);

  async function submitRemote(name: string, url: string): Promise<void> {
    if (repoId === null) return;
    remoteBusy = true;
    try {
      if (remoteEditName) {
        await git.setRemoteUrl(repoId, remoteEditName, url);
        showToast("success", t("refs.remote.urlSet", { name: remoteEditName }));
      } else {
        await git.addRemote(repoId, name, url);
        showToast("success", t("refs.remote.added", { name }));
      }
      remoteOpen = false;
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    } finally {
      remoteBusy = false;
    }
  }

  let removeRemoteOpen = $state(false);
  let removeRemoteTarget = $state("");

  async function confirmRemoveRemote(): Promise<void> {
    if (repoId === null) return;
    try {
      await git.removeRemote(repoId, removeRemoteTarget);
      showToast("success", t("refs.remote.removed", { name: removeRemoteTarget }));
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    }
  }

  // ===================== pull / push / merge / rebase =====================
  let pushOpen = $state(false);
  let pushBusy = $state(false);

  async function doPush(
    remote: string,
    branch: string,
    o: { forceWithLease: boolean; setUpstream: boolean; tags: boolean }
  ): Promise<void> {
    if (repoId === null) return;
    pushBusy = true;
    try {
      await runPush(repoId, remote, branch, o);
      await loadRefsData(repoId);
    } finally {
      pushBusy = false;
    }
  }

  let pullOpen = $state(false);
  let pullBusy = $state(false);

  async function doPull(
    remote: string | null,
    branch: string | null,
    mode: string
  ): Promise<void> {
    if (repoId === null) return;
    pullBusy = true;
    try {
      await runPull(repoId, remote, branch, mode as "merge" | "rebase" | "ff_only");
      await loadRefsData(repoId);
    } finally {
      pullBusy = false;
    }
  }

  let mergeOpen = $state(false);
  let mergeMode = $state<"merge" | "rebase">("merge");
  let mergeInitialTarget = $state("");
  let mergeBusy = $state(false);

  async function doMergeRebase(
    mode: "merge" | "rebase",
    target: string,
    ffOnly: boolean
  ): Promise<void> {
    if (repoId === null) return;
    mergeBusy = true;
    try {
      if (mode === "merge") {
        const res = await git.merge(repoId, target, ffOnly);
        if (res.success) {
          showToast("success", t("refs.merge.done", { target }));
        } else if (!(await conflictEntered(repoId))) {
          // P8: conflict → the banner takes over; only a real failure toasts.
          showToast("error", res.message || t("refs.merge.failed"));
        }
      } else {
        try {
          await git.rebase(repoId, target);
          showToast("success", t("refs.rebase.done", { target }));
        } catch (e) {
          if (!(await conflictEntered(repoId))) throw e;
        }
      }
      await refreshAll();
    } finally {
      mergeBusy = false;
    }
  }

  // ===================== stash =====================
  let stashDiffOpen = $state(false);
  let stashDiffEntry = $state<StashEntry | null>(null);

  let stashDropOpen = $state(false);
  let stashDropIndex = $state(0);

  async function confirmStashDrop(): Promise<void> {
    if (repoId === null) return;
    try {
      await git.stashDrop(repoId, stashDropIndex);
      showToast("success", t("refs.stash.dropped"));
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    }
  }

  // ===================== reflog restore =====================
  let reflogRestoreOpen = $state(false);
  let reflogRestoreTarget = $state<{ hash: string; subject: string } | null>(null);

  function askReflogRestore(hash: string, subject: string): void {
    reflogRestoreTarget = { hash, subject };
    reflogRestoreOpen = true;
  }

  async function confirmReflogRestore(): Promise<void> {
    const t0 = reflogRestoreTarget;
    if (repoId === null || !t0) return;
    try {
      const undo = await git.reset(repoId, "hard", t0.hash);
      lastReset = { backupRef: undo.backup_ref, mode: "hard", snapshotId: undo.snapshot_id };
      showToast(
        "success",
        t("refs.reflog.restored", { hash: t0.hash.slice(0, 7) }),
        t("refs.reset.undoHint"),
        10000,
        { label: t("refs.reset.undo"), run: () => void undoReset() }
      );
      await refreshAll();
    } catch (e) {
      normalizeError(e);
    }
  }
</script>

<!-- branch dialogs -->
<InputDialog
  bind:open={newBranchOpen}
  bind:value={newBranchValue}
  title={t("refs.newBranch.title")}
  description={newBranchStart
    ? t("refs.newBranch.startDesc", { start: newBranchStart })
    : ""}
  label={t("history.branchName")}
  placeholder="feature/…"
  busy={branchBusy}
  validate={(v) => (/[\s~^:?*\[\\]{2,}/.test(v) ? t("refs.errors.badRefName") : null)}
  onsubmit={submitNewBranch}
/>
<InputDialog
  bind:open={renameOpen}
  bind:value={renameValue}
  title={t("refs.rename.title", { name: renameTarget })}
  label={t("refs.rename.label")}
  busy={branchBusy}
  onsubmit={submitRename}
/>
<InputDialog
  bind:open={upstreamOpen}
  bind:value={upstreamValue}
  title={t("refs.upstream.title", { name: upstreamTarget })}
  label={t("refs.upstream.label")}
  placeholder="origin/main"
  busy={branchBusy}
  onsubmit={submitUpstream}
/>

<ConfirmDialog
  bind:open={deleteBranchOpen}
  title={t("refs.deleteBranch.title", { name: deleteBranchTarget ?? "" })}
  description={t("refs.deleteBranch.forceDesc")}
  confirmLabel={t("refs.deleteBranch.forceConfirm")}
  destructive
  onconfirm={confirmDeleteBranch}
>
  <p class="rounded-md bg-warning-surface px-2.5 py-2 text-xs text-warning dark:text-warning">
    {t("refs.deleteBranch.forceWarn")}
  </p>
</ConfirmDialog>

<!-- reset / clean / backups -->
<ResetDialog
  bind:open={resetOpen}
  initialTarget={resetInitialTarget}
  branches={active?.branches ?? []}
  busy={resetBusy}
  onexecute={executeReset}
/>
<CleanDialog
  bind:open={cleanOpen}
  paths={cleanPaths}
  loading={cleanLoading}
  busy={cleanBusy}
  onconfirm={confirmClean}
/>
<BackupListDialog
  bind:open={backupsOpen}
  refs={backupRefs}
  loading={backupsLoading}
  busy={backupsBusy}
  ondelete={deleteBackups}
  onrestore={(name) => askReflogRestore(name, name)}
/>

<!-- tags -->
<NewTagDialog bind:open={tagOpen} target={tagTarget} busy={tagBusy} oncreate={createTag} />
<ConfirmDialog
  bind:open={deleteTagOpen}
  title={t("refs.deleteTag.title", { name: deleteTagTarget })}
  description={t("refs.deleteTag.desc")}
  confirmLabel={t("common.delete")}
  destructive
  onconfirm={confirmDeleteTag}
/>

<!-- remotes -->
<RemoteDialog
  bind:open={remoteOpen}
  editName={remoteEditName}
  initialUrl={remoteEditUrl}
  busy={remoteBusy}
  onsubmit={submitRemote}
/>
<ConfirmDialog
  bind:open={removeRemoteOpen}
  title={t("refs.remote.removeTitle", { name: removeRemoteTarget })}
  description={t("refs.remote.removeDesc")}
  confirmLabel={t("common.delete")}
  destructive
  onconfirm={confirmRemoveRemote}
/>

<!-- pull / push / merge / rebase -->
<PushDialog
  bind:open={pushOpen}
  remotes={refsData.remotes}
  branch={active?.branch ?? ""}
  hasUpstream={!!active?.branches.find((b) => b.current)?.upstream}
  busy={pushBusy}
  onpush={doPush}
/>
<PullDialog
  bind:open={pullOpen}
  remotes={refsData.remotes}
  branches={active?.branches ?? []}
  currentBranch={active?.branch ?? ""}
  upstream={active?.branches.find((b) => b.current)?.upstream ?? null}
  busy={pullBusy}
  onpull={doPull}
/>
<MergeRebaseDialog
  bind:open={mergeOpen}
  bind:mode={mergeMode}
  branches={active?.branches ?? []}
  currentBranch={active?.branch ?? ""}
  initialTarget={mergeInitialTarget}
  {repoId}
  busy={mergeBusy}
  onexecute={doMergeRebase}
/>

<!-- compare -->
<CompareDialog
  bind:open={compareOpen}
  branches={active?.branches ?? []}
  tags={refsData.tags}
  initialLeft={compareLeft}
  initialRight={compareRight}
  {repoId}
/>

<!-- stash -->
<StashDiffDialog bind:open={stashDiffOpen} entry={stashDiffEntry} {repoId} />
<ConfirmDialog
  bind:open={stashDropOpen}
  title={t("refs.stash.dropTitle", { index: stashDropIndex })}
  description={t("refs.stash.dropDesc")}
  confirmLabel={t("common.delete")}
  destructive
  onconfirm={confirmStashDrop}
/>

<!-- reflog restore -->
<ConfirmDialog
  bind:open={reflogRestoreOpen}
  title={t("refs.reflog.restoreTitle")}
  description={t("refs.reflog.restoreDesc", {
    hash: reflogRestoreTarget?.hash.slice(0, 7) ?? "",
  })}
  confirmLabel={t("refs.reflog.restoreConfirm")}
  destructive
  onconfirm={() => {
    void confirmReflogRestore();
    reflogRestoreTarget = null;
  }}
>
  <div class="rounded-md border p-2 text-xs text-muted-foreground">
    {reflogRestoreTarget?.subject}
  </div>
</ConfirmDialog>
