<script lang="ts">
  // Says whether this device trusts the share's https origins, and offers the
  // share's certificate where it doesn't. The browsers never tell the app
  // about a certificate error, so the share's pane checks itself each time it
  // shows the share.
  import * as api from "$lib/api";
  import type { CertificateStatus, Share } from "$lib/api";
  import { certificateStoreAsks } from "$lib/platform";

  let { share }: { share: Share } = $props();

  let status = $state<CertificateStatus | null>(null);
  let checkFailed = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let savedTo = $state<string | null>(null);

  const name = $derived(share.name ? `“${share.name}”` : "This share");
  const installable = $derived(
    status?.certificates.find((c) => c.installable && !c.installed) ?? null,
  );
  const installed = $derived(status?.certificates.some((c) => c.installed) ?? false);
  const other = $derived(status?.certificates.find((c) => !c.installable) ?? null);
  // Every origin listed is https, so the scheme says nothing.
  const untrusted = $derived(
    (status?.untrustedOrigins ?? []).map((origin) => origin.replace(/^https:\/\//, "")).join(", "),
  );
  const isOrAre = $derived(status?.untrustedOrigins.length === 1 ? "isn't" : "aren't");

  // The list reloads hand over a new `share` object, for one whenever an app
  // opens, so the check follows the id and not the object.
  const shareId = $derived(share.id);

  $effect(() => {
    const id = shareId;
    status = null;
    checkFailed = false;
    error = null;
    savedTo = null;
    void check(id);
  });

  async function check(id: string) {
    try {
      const checked = await api.checkCertificates(id);
      if (id === shareId) {
        status = checked;
        checkFailed = checked.unchecked;
      }
    } catch (e) {
      console.warn(`checking share ${id}'s certificates failed`, e);
      if (id === shareId) checkFailed = true;
    }
  }

  async function install() {
    busy = true;
    error = null;
    try {
      await api.installCertificates(share.id);
      await check(share.id);
    } catch (e) {
      error = String(e) === "cancelled" ? null : String(e);
    } finally {
      busy = false;
    }
  }

  async function checkAgain() {
    busy = true;
    try {
      await check(share.id);
    } finally {
      busy = false;
    }
  }

  async function save() {
    error = null;
    try {
      savedTo = await api.saveCertificates(share.id);
    } catch (e) {
      error = String(e);
    }
  }
</script>

{#if checkFailed}
  <div class="mt-8 rounded-2xl bg-info-card p-4">
    <h2 class="font-semibold">Wispers Access couldn't check the apps' certificates</h2>
    <div class="mt-1 flex flex-col gap-2 text-[14px] leading-relaxed text-on-surface-variant">
      <p>The share's host didn't answer, or couldn't reach the apps' servers.</p>
      <div>
        <button
          type="button"
          class="mt-1 h-10 rounded-full bg-primary px-5 text-sm font-semibold text-primary-dark hover:bg-[#95c877] disabled:opacity-60"
          disabled={busy}
          onclick={checkAgain}
        >
          Check again
        </button>
      </div>
    </div>
  </div>
{:else if status && status.untrustedOrigins.length > 0}
  <div class="mt-8 rounded-2xl bg-info-card p-4">
    <h2 class="font-semibold">This share's apps need a certificate</h2>
    <div class="mt-1 flex flex-col gap-2 text-[14px] leading-relaxed text-on-surface-variant">
      {#if installable}
        {#if installable.kind === "ca"}
          <p>
            {name} comes with a certificate of “{installable.subject}”, which you'll have to install
            to make this device trust the apps' servers. Only install it if you trust this share. Once
            it's installed, the certificate's owner can make any website show up as secure.
          </p>
        {:else}
          <p>
            {name} comes with a certificate for {installable.names.join(", ") ||
              installable.subject}, which you'll have to install to make this device trust that
            server. It vouches for nothing else.
          </p>
        {/if}
        {#if status.canInstall}
          <p>{certificateStoreAsks} Browsers already open may need restarting.</p>
          <div>
            <button
              type="button"
              class="mt-1 h-10 rounded-full bg-primary px-5 text-sm font-semibold text-primary-dark hover:bg-[#95c877] disabled:opacity-60"
              disabled={busy}
              onclick={install}
            >
              Install certificate
            </button>
          </div>
        {:else}
          <p>
            Save it, then import it in your browser's certificate settings as an authority it trusts
            for websites.
          </p>
          <div>
            <button
              type="button"
              class="mt-1 h-10 rounded-full bg-primary px-5 text-sm font-semibold text-primary-dark hover:bg-[#95c877]"
              onclick={save}
            >
              Save certificate…
            </button>
          </div>
          {#if savedTo}
            <p class="select-text">Saved to {savedTo}.</p>
          {/if}
        {/if}
      {:else if installed}
        <p>
          The certificate is installed, but {untrusted} still {isOrAre} trusted. Whoever runs the
          share has to fix this.
        </p>
      {:else if other}
        <p>
          {name} comes with a certificate for {other.names.join(", ") || other.subject} instead of its
          CA's. Whoever runs the share has to add the CA's certificate.
        </p>
      {:else}
        <p>
          This device doesn't trust the certificate of {untrusted}, and the share
          has none to install. Whoever runs the share has to fix this.
        </p>
      {/if}
      {#if error}
        <p class="text-destructive select-text" role="alert">{error}</p>
      {/if}
    </div>
  </div>
{/if}
