<script lang="ts">
  import type { Share } from "$lib/api";

  let {
    share,
    onconfirm,
    onclose,
  }: { share: Share; onconfirm: () => void; onclose: () => void } = $props();

  let dialog: HTMLDialogElement | undefined = $state();

  $effect(() => {
    dialog?.showModal();
  });

  function onclick(event: MouseEvent) {
    if (event.target === dialog) onclose();
  }
</script>

<dialog
  bind:this={dialog}
  class="m-auto w-[400px] rounded-2xl bg-background p-6 text-on-surface shadow-xl"
  aria-labelledby="remove-title"
  {onclick}
  onclose={onclose}
>
  <h2 id="remove-title" class="font-serif text-[22px]">Remove {share.name || "this share"}?</h2>
  <p class="mt-3 text-[14px] leading-relaxed text-on-surface-variant">
    This device's access will be removed on the host. You'll need a new invitation code to
    rejoin.
  </p>
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
      class="h-10 rounded-full bg-destructive px-6 text-sm font-semibold text-white hover:bg-[#9c1f18]"
      onclick={onconfirm}
    >
      Remove
    </button>
  </div>
</dialog>
