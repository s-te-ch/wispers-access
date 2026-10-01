<script lang="ts">
  import type { ShareManager } from "$lib/shares.svelte";
  import JoinForm from "./JoinForm.svelte";

  // The native dialog brings the backdrop, focus trapping and Escape.
  let { manager, onclose }: { manager: ShareManager; onclose: () => void } = $props();

  let dialog: HTMLDialogElement | undefined = $state();

  $effect(() => {
    dialog?.showModal();
  });

  function onclick(event: MouseEvent) {
    // A click on the backdrop lands on the dialog element itself.
    if (event.target === dialog) onclose();
  }
</script>

<dialog
  bind:this={dialog}
  class="m-auto w-[440px] rounded-2xl bg-background p-6 text-on-surface shadow-xl backdrop:bg-black/35"
  aria-labelledby="add-share-title"
  {onclick}
  onclose={onclose}
>
  <div class="mb-5 flex items-center justify-between">
    <h2 id="add-share-title" class="font-serif text-[22px]">Add a share</h2>
    <button
      type="button"
      class="flex h-8 w-8 items-center justify-center rounded-full text-xl leading-none text-on-surface-variant hover:bg-black/[0.06]"
      aria-label="Close"
      onclick={onclose}
    >
      ×
    </button>
  </div>
  <JoinForm {manager} onjoined={onclose}>
    {#snippet actions({ canJoin, joining, join })}
      <div class="mt-6 flex justify-end gap-2">
        <button
          type="button"
          class="h-10 rounded-full px-5 text-sm font-semibold hover:bg-black/[0.06]"
          onclick={onclose}
        >
          Cancel
        </button>
        <button
          type="button"
          class="h-10 rounded-full bg-primary px-6 text-sm font-semibold text-primary-dark hover:bg-[#95c877] disabled:opacity-50 disabled:hover:bg-primary"
          disabled={!canJoin}
          onclick={join}
        >
          {joining ? "Joining…" : "Join"}
        </button>
      </div>
    {/snippet}
  </JoinForm>
</dialog>
