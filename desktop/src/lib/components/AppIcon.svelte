<script lang="ts">
  import type { SharedApp } from "$lib/api";

  // A stand-in until icons are harvested through the proxy: a tile in a colour
  // the app's name picks, with its initial. Known kinds get their brand's hue.
  // TODO: Implement icon harvesting in the SDK and replace this workaround.
  let { app, size = 40 }: { app: SharedApp; size?: number } = $props();

  const palette = ["#c2622d", "#4a6fa5", "#6b5b95", "#2f8f6b", "#b4883a", "#8f4f6a"];

  const color = $derived.by(() => {
    if (app.kind === "jellyfin") return "#7b5ea7";
    if (app.kind === "immich") return "#4250af";
    let hash = 0;
    for (const ch of app.name) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
    return palette[hash % palette.length];
  });

  const initial = $derived(app.name.trim().charAt(0).toUpperCase() || "?");
</script>

<span
  class="inline-flex shrink-0 items-center justify-center rounded-lg font-semibold text-white"
  style="width: {size}px; height: {size}px; background: {color}; font-size: {size * 0.45}px"
  aria-hidden="true"
>
  {initial}
</span>
