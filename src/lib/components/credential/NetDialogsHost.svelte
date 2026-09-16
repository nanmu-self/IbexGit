<script lang="ts">
  /**
   * NetDialogsHost (P7)：全局挂载 P7 网络对话框（克隆 / 新建仓库 /
   * 凭据管理 / 凭据三动作框），并在挂载时接好事件订阅。
   */
  import CredentialDialog from "./CredentialDialog.svelte";
  import CloneDialog from "./CloneDialog.svelte";
  import NewRepoDialog from "./NewRepoDialog.svelte";
  import NetworkDialog from "./NetworkDialog.svelte";
  import { netDialogs, wireNetEvents } from "$lib/stores/netdialogs.svelte";

  wireNetEvents();
</script>

<CloneDialog bind:open={netDialogs.cloneOpen} />
<NewRepoDialog bind:open={netDialogs.newRepoOpen} />
<NetworkDialog bind:open={netDialogs.credentialsOpen} />
<CredentialDialog />
