<script lang="ts">
  import type { ShareManager } from "$lib/shares.svelte";
  import ShareRow from "./ShareRow.svelte";

  let { manager, onadd }: { manager: ShareManager; onadd: () => void } = $props();
</script>

<!-- The window has no title bar on macOS, so the whole sidebar drags it,
     buttons excepted (Tauri's "deep" drag region skips them). -->
<nav
  aria-label="Shares"
  class="flex h-full w-[300px] shrink-0 flex-col gap-7 border-r border-sidebar-edge bg-sidebar px-3.5 pt-8 pb-4"
  data-tauri-drag-region="deep"
>
  <img src="/wispers-access-text.svg" alt="Wispers Access" class="mx-2.5 w-[150px]" draggable="false" />

  <div
    class="flex justify-between px-2.5 text-[11px] font-semibold tracking-[0.16em] text-on-surface-variant uppercase"
  >
    <span>Shared with you</span>
    {#if manager.shares.length > 0}
      <span>{manager.shares.length}</span>
    {/if}
  </div>

  <div class="-mt-4 flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto">
    {#if manager.loaded && manager.shares.length === 0}
      <p class="px-2.5 text-[13px] text-on-surface-variant">
        Nothing yet. Shares you join appear here.
      </p>
    {/if}
    {#each manager.shares as share (share.id)}
      <ShareRow
        {share}
        availability={manager.availability(share)}
        selected={share.id === manager.selectedId}
        onselect={() => manager.select(share.id)}
      />
    {/each}
  </div>

  <button
    type="button"
    class="flex h-11 w-full items-center justify-center gap-2 rounded-full bg-primary font-semibold text-primary-dark transition-colors hover:bg-[#95c877]"
    onclick={onadd}
  >
    <span aria-hidden="true" class="text-lg leading-none">+</span>
    Add a share
  </button>
</nav>
