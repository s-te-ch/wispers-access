<script lang="ts">
  import type { Share } from "$lib/api";
  import type { ShareManager } from "$lib/shares.svelte";
  import {
    describeAvailability,
    describeTransport,
    explainTerminalState,
    isoDate,
  } from "$lib/format";
  import AppIcon from "./AppIcon.svelte";
  import ConfirmRemoveDialog from "./ConfirmRemoveDialog.svelte";
  import StatusDot from "./StatusDot.svelte";

  let { share, manager }: { share: Share; manager: ShareManager } = $props();

  const availability = $derived(manager.availability(share));

  let confirmingRemoval = $state(false);
  let openError = $state<string | null>(null);

  async function open(appId: string) {
    openError = null;
    try {
      await manager.openApp(share.id, appId);
    } catch (e) {
      openError = String(e);
    }
  }

  async function remove() {
    confirmingRemoval = false;
    await manager.leave(share.id);
  }
</script>

<div class="mx-auto max-w-[680px] pt-[72px] pb-16">
  <div class="flex items-center gap-2 text-[11px] font-semibold tracking-[0.16em] text-on-surface-variant uppercase">
    <StatusDot {availability} size={8} />
    {describeAvailability(availability)}
  </div>
  <h1 class="mt-2 font-serif text-[40px] leading-tight select-text">{share.name || "Untitled share"}</h1>

  <div class="mt-6 grid grid-cols-3 gap-3">
    {@render tile("Last connected", share.lastConnectedMs === null ? "—" : isoDate(share.lastConnectedMs))}
    {@render tile("Joined", isoDate(share.joinedAtMs))}
    {@render tile("Connection", describeTransport(share.transport))}
  </div>

  {#if share.state !== "live"}
    <div class="mt-8 rounded-2xl bg-info-card p-4">
      <h2 class="font-semibold">This share is no longer available</h2>
      <p class="mt-1 text-[14px] leading-relaxed text-on-surface-variant">
        {explainTerminalState(share.state)}
      </p>
    </div>
  {:else}
    <h2 class="mt-9 mb-3 text-[11px] font-semibold tracking-[0.16em] text-on-surface-variant uppercase">
      Apps
    </h2>
    {#if share.apps.length === 0}
      <p class="text-[14px] text-on-surface-variant">
        No apps shared yet. They appear here once the host adds some.
      </p>
    {/if}
    <ul class="flex flex-col gap-3">
      {#each share.apps as app (app.id)}
        <li class="flex items-center gap-4 rounded-2xl bg-surface p-3 pl-4 shadow-card">
          <AppIcon {app} size={36} />
          <span class="flex min-w-0 flex-1 flex-col">
            <span class="truncate font-serif text-[17px]">{app.name}</span>
            <span class="truncate text-xs text-on-surface-variant select-text">{app.host}</span>
          </span>
          <button
            type="button"
            class="flex h-10 shrink-0 items-center gap-1.5 rounded-full bg-primary px-4 text-sm font-semibold text-primary-dark hover:bg-[#95c877]"
            onclick={() => open(app.id)}
          >
            Open in browser
            <span aria-hidden="true">↗</span>
          </button>
        </li>
      {/each}
    </ul>
    {#if openError}
      <p class="mt-3 text-[13px] text-destructive select-text" role="alert">{openError}</p>
    {/if}
  {/if}

  <button
    type="button"
    class="mt-10 h-11 rounded-full border border-outline px-6 text-sm font-semibold text-destructive hover:bg-destructive/5"
    onclick={() => (confirmingRemoval = true)}
  >
    Remove from this device
  </button>
</div>

{#if confirmingRemoval}
  <ConfirmRemoveDialog {share} onconfirm={remove} onclose={() => (confirmingRemoval = false)} />
{/if}

{#snippet tile(label: string, value: string)}
  <div class="rounded-[14px] bg-info-card px-4 py-3">
    <div class="text-[10px] font-semibold tracking-[0.16em] text-on-surface-variant uppercase">
      {label}
    </div>
    <div class="mt-1 text-[16px] select-text">{value}</div>
  </div>
{/snippet}
