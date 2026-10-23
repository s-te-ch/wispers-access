<script lang="ts">
  // Says whether this device trusts the share's https origins, and offers the
  // share's certificate where it doesn't. The browsers never tell the app
  // about a certificate error, so the share's pane checks itself each time it
  // shows the share.
  import * as api from "$lib/api";
  import type { CertificateStatus, Share } from "$lib/api";

  let { share }: { share: Share } = $props();

  let status = $state<CertificateStatus | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let savedTo = $state<string | null>(null);
  let installed = $state(false);

  const name = $derived(share.name || "This share");
  const installable = $derived(status?.certificates.find((c) => c.installable) ?? null);
  const other = $derived(status?.certificates.find((c) => !c.installable) ?? null);
  const askedBy = navigator.userAgent.includes("Mac")
    ? "macOS asks for your password."
    : "Windows asks you to confirm.";

  $effect(() => {
    const id = share.id;
    status = null;
    error = null;
    savedTo = null;
    installed = false;
    void check(id);
  });

  async function check(id: string) {
    try {
      const checked = await api.checkCertificates(id);
      if (id === share.id) status = checked;
    } catch (e) {
      console.warn(`checking share ${id}'s certificates failed`, e);
    }
  }

  async function install() {
    busy = true;
    error = null;
    try {
      await api.installCertificates(share.id);
      installed = true;
      await check(share.id);
    } catch (e) {
      error = String(e) === "cancelled" ? null : String(e);
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

{#if status && status.untrustedOrigins.length > 0}
  <div class="mt-8 rounded-2xl bg-info-card p-4">
    <h2 class="font-semibold">This share's apps need a certificate</h2>
    <div class="mt-1 flex flex-col gap-2 text-[14px] leading-relaxed text-on-surface-variant">
      {#if installed}
        <p>
          The certificate is installed, but {status.untrustedOrigins.join(", ")} still isn't trusted.
          Whoever runs the share has to fix this.
        </p>
      {:else if installable}
        {#if installable.kind === "ca"}
          <p>
            {name} comes with the certificate of “{installable.subject}”, which this device needs to
            trust the apps' servers. Once it's installed, whoever holds it could vouch for any website
            on this device. Only install it if you trust {name}.
          </p>
        {:else}
          <p>
            {name} comes with the certificate of {installable.names.join(", ") || installable.subject},
            which this device needs to trust that server. It vouches for nothing else.
          </p>
        {/if}
        {#if status.canInstall}
          <p>{askedBy} Browsers already open may need restarting to trust it.</p>
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
      {:else if other}
        <p>
          {name} comes with the certificate of {other.names.join(", ") || other.subject} instead of its
          CA's. Whoever runs the share has to add the CA's certificate.
        </p>
      {:else}
        <p>
          This device doesn't trust the certificate of {status.untrustedOrigins.join(", ")}, and {name}
          has none to install. Whoever runs the share has to fix this.
        </p>
      {/if}
      {#if error}
        <p class="text-destructive select-text" role="alert">{error}</p>
      {/if}
    </div>
  </div>
{/if}
