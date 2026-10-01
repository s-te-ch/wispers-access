<script lang="ts">
  import type { UpdateChecker } from "$lib/updates.svelte";

  // A newer version is published: say so, and install it on request.
  let { updates }: { updates: UpdateChecker } = $props();
</script>

{#if updates.available}
  <div class="rounded-xl bg-surface p-3 text-[13px] shadow-card" role="status">
    <p class="font-semibold">Wispers Access {updates.available.version} is available</p>
    {#if updates.installing}
      <div class="mt-2 h-1.5 overflow-hidden rounded-full bg-outline" aria-hidden="true">
        <div class="h-full bg-primary-dark transition-[width]" style="width: {updates.progress * 100}%"></div>
      </div>
      <p class="mt-1.5 text-on-surface-variant">
        {updates.progress < 1 ? "Downloading…" : "Installing…"}
      </p>
    {:else}
      <div class="mt-2 flex gap-2">
        <button
          type="button"
          class="h-8 rounded-full bg-primary px-3 text-xs font-semibold text-primary-dark hover:bg-[#95c877]"
          onclick={() => updates.install()}
        >
          Install and restart
        </button>
        <button
          type="button"
          class="h-8 rounded-full px-3 text-xs font-semibold hover:bg-black/[0.06]"
          onclick={() => updates.dismiss()}
        >
          Later
        </button>
      </div>
    {/if}
    {#if updates.error}
      <p class="mt-2 text-destructive select-text" role="alert">{updates.error}</p>
    {/if}
  </div>
{/if}
