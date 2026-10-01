<script lang="ts">
  import type { Availability, Share } from "$lib/api";
  import { ago, count, describeAvailability } from "$lib/format";
  import AppIcon from "./AppIcon.svelte";
  import StatusDot from "./StatusDot.svelte";

  let {
    share,
    availability,
    selected,
    onselect,
  }: {
    share: Share;
    availability: Availability | "checking";
    selected: boolean;
    onselect: () => void;
  } = $props();

  const dimmed = $derived(availability === "offline" || share.state !== "live");

  const summary = $derived.by(() => {
    const status = describeAvailability(availability);
    const when =
      share.lastConnectedMs !== null && share.state === "live"
        ? ` ${ago(share.lastConnectedMs)}`
        : "";
    return `${count(share.apps.length, "app")} · ${status}${when}`;
  });
</script>

<button
  type="button"
  class="flex w-full items-center gap-3 rounded-xl px-3 py-3 text-left transition-colors
    {selected ? 'bg-surface shadow-card' : 'hover:bg-black/[0.04]'}
    {dimmed && !selected ? 'opacity-60' : ''}"
  aria-current={selected ? "true" : undefined}
  onclick={onselect}
>
  <span class="flex min-w-0 flex-1 flex-col gap-0.5">
    <span class="flex items-center gap-2">
      <StatusDot {availability} />
      <span class="truncate font-serif text-[17px] leading-tight">{share.name || "Untitled share"}</span>
    </span>
    <span class="truncate pl-4 text-xs text-on-surface-variant">{summary}</span>
  </span>
  {#if share.apps.length > 0}
    <!-- The tiles overlap a little, each outlined in the row's background so
         they read as a stack rather than one blob. -->
    <span class="flex shrink-0 -space-x-1.5">
      {#each share.apps.slice(0, 3) as app (app.id)}
        <span class="inline-flex rounded-lg ring-2 {selected ? 'ring-surface' : 'ring-sidebar'}">
          <AppIcon {app} size={20} />
        </span>
      {/each}
    </span>
  {/if}
</button>
