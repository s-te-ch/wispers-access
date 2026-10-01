<script lang="ts">
  import type { ShareManager } from "$lib/shares.svelte";
  import JoinForm from "./JoinForm.svelte";

  // No shares yet. Show the join form inline, where the detail pane will be.
  let { manager }: { manager: ShareManager } = $props();
</script>

<div class="mx-auto max-w-[400px] pt-[120px]">
  <h1 class="font-serif text-[32px] leading-tight">Join a share</h1>
  <p class="mt-3 mb-7 text-[15px] leading-relaxed text-on-surface-variant">
    Paste the invitation code you were sent. The apps behind it show up on the left once you
    have joined.
  </p>
  <JoinForm {manager} onjoined={() => {}}>
    {#snippet actions({ canJoin, joining, join })}
      <button
        type="button"
        class="mt-6 h-11 w-full rounded-full bg-primary font-semibold text-primary-dark hover:bg-[#95c877] disabled:opacity-50 disabled:hover:bg-primary"
        disabled={!canJoin}
        onclick={join}
      >
        {joining ? "Joining…" : "Join"}
      </button>
    {/snippet}
  </JoinForm>
</div>
