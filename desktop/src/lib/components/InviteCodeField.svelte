<script lang="ts">
  import { clipboardInvite } from "$lib/api";
  import { onMount } from "svelte";

  // The invitation code field with its paste button. Prefilled from the
  // clipboard if it holds a code.
  let {
    value = $bindable(""),
    disabled = false,
    onsubmit,
  }: { value?: string; disabled?: boolean; onsubmit?: () => void } = $props();

  let textarea: HTMLTextAreaElement | undefined = $state();

  onMount(async () => {
    if (value === "") value = (await clipboardInvite()) ?? "";
  });

  async function paste() {
    value = (await clipboardInvite()) ?? value;
    textarea?.focus();
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      onsubmit?.();
    }
  }
</script>

<label class="flex flex-col gap-2">
  <span class="text-[11px] font-semibold tracking-[0.16em] text-on-surface-variant uppercase">
    Invitation code
  </span>
  <!-- The field is what the form is for, so it takes focus even inside the
       modal, which would otherwise focus its close button. -->
  <!-- svelte-ignore a11y_autofocus -->
  <textarea
    autofocus
    bind:this={textarea}
    bind:value
    {disabled}
    {onkeydown}
    rows="3"
    spellcheck="false"
    autocomplete="off"
    autocapitalize="off"
    placeholder="wax1_…"
    class="w-full resize-none rounded-xl border border-outline bg-surface px-4 py-3 font-mono text-[13px] leading-relaxed text-on-surface select-text focus:border-primary-dark focus:outline-none disabled:opacity-60"
  ></textarea>
</label>
<div class="mt-2 flex items-center justify-between gap-4">
  <span class="text-xs text-on-surface-variant">
    Codes are issued by the person sharing the app with you.
  </span>
  <button
    type="button"
    {disabled}
    class="shrink-0 rounded-full bg-black/[0.06] px-3 py-1.5 text-xs font-semibold hover:bg-black/[0.1] disabled:opacity-60"
    onclick={paste}
  >
    Paste
  </button>
</div>
