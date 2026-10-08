<script lang="ts">
  import { autostartEnabled, setAutostartEnabled, windowPolicy } from "$lib/api";
  import { onMount } from "svelte";

  // The autostart setting, where the window carries it: on Linux
  // desktops without a tray. Everywhere else a menu has it and this shows
  // nothing. Null until known, or where a menu has it.
  let enabled = $state<boolean | null>(null);
  let error = $state<string | null>(null);

  onMount(async () => {
    if ((await windowPolicy()) === "quitOnClose") enabled = await autostartEnabled();
  });

  async function toggle() {
    if (enabled === null) return;
    const wanted = !enabled;
    error = null;
    try {
      await setAutostartEnabled(wanted);
      enabled = wanted;
    } catch (e) {
      error = String(e);
    }
  }
</script>

{#if enabled !== null}
  <div class="-mt-3 px-2.5 text-[13px]">
    <button
      type="button"
      role="switch"
      aria-checked={enabled}
      class="flex w-full items-center justify-between gap-3 py-1 text-on-surface-variant"
      onclick={toggle}
    >
      Launch at login
      <span
        aria-hidden="true"
        class="relative h-5 w-9 shrink-0 rounded-full transition-colors {enabled ? 'bg-primary-dark' : 'bg-outline'}"
      >
        <span
          class="absolute top-0.5 left-0.5 size-4 rounded-full bg-surface shadow transition-transform {enabled
            ? 'translate-x-4'
            : ''}"
        ></span>
      </span>
    </button>
    {#if error}
      <p class="mt-1 text-destructive select-text" role="alert">{error}</p>
    {/if}
  </div>
{/if}
