<script lang="ts">
  import type { ShareManager } from "$lib/shares.svelte";
  import InviteCodeField from "./InviteCodeField.svelte";
  import type { Snippet } from "svelte";

  // The join form itself, with code field and the error under it. The dialog
  // and the first-run pane wrap this with their own buttons.
  let {
    manager,
    onjoined,
    actions,
  }: {
    manager: ShareManager;
    onjoined: () => void;
    /** The buttons, given whether a join is possible and the join to call. */
    actions: Snippet<[{ canJoin: boolean; joining: boolean; join: () => void }]>;
  } = $props();

  let code = $state("");
  let joining = $state(false);
  let error = $state<string | null>(null);

  const canJoin = $derived(code.trim() !== "" && !joining);

  async function join() {
    if (!canJoin) return;
    joining = true;
    error = null;
    try {
      await manager.join(code);
      onjoined();
    } catch (e) {
      error = String(e);
    } finally {
      joining = false;
    }
  }

  $effect(() => {
    // A new code is a new attempt.
    void code;
    error = null;
  });
</script>

<InviteCodeField bind:value={code} disabled={joining} onsubmit={join} />
{#if error}
  <p class="mt-3 text-[13px] text-destructive select-text" role="alert">{error}</p>
{/if}
{@render actions({ canJoin, joining, join })}
