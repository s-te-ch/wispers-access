<script lang="ts">
  import { ShareManager } from "$lib/shares.svelte";
  import { UpdateChecker } from "$lib/updates.svelte";
  import AddShareDialog from "$lib/components/AddShareDialog.svelte";
  import FirstRun from "$lib/components/FirstRun.svelte";
  import ShareDetail from "$lib/components/ShareDetail.svelte";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import { dragRegion } from "$lib/platform";
  import { onMount } from "svelte";

  const manager = new ShareManager();
  const updates = new UpdateChecker();
  let adding = $state(false);

  onMount(() => {
    void manager.start();
    updates.start();
  });

  function onkeydown(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key === "n") {
      event.preventDefault();
      adding = true;
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="flex h-dvh w-full">
  <Sidebar {manager} {updates} onadd={() => (adding = true)} />
  <main class="relative min-w-0 flex-1 overflow-y-auto px-10">
    {#if dragRegion}
      <!-- The title bar's worth of the detail pane drags the window too. -->
      <div class="absolute inset-x-0 top-0 h-7" data-tauri-drag-region={dragRegion}></div>
    {/if}
    {#if manager.selected}
      {#key manager.selected.id}
        <ShareDetail share={manager.selected} {manager} />
      {/key}
    {:else if manager.loaded}
      <FirstRun {manager} />
    {/if}
  </main>
</div>

{#if adding}
  <AddShareDialog {manager} onclose={() => (adding = false)} />
{/if}
