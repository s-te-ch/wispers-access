<script lang="ts">
  import type { Availability } from "$lib/api";

  let { availability, size = 8 }: { availability: Availability | "checking"; size?: number } =
    $props();

  const color = $derived.by(() => {
    switch (availability) {
      case "online":
        return "bg-online";
      case "removed":
      case "revoked":
        return "bg-destructive";
      case "checking":
        return "bg-offline animate-pulse";
      default:
        return "bg-offline";
    }
  });
</script>

<span
  class="inline-block shrink-0 rounded-full {color}"
  style="width: {size}px; height: {size}px"
  aria-hidden="true"
></span>
