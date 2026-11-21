<script lang="ts">
  import { api, message, type Chain } from "../lib/api";
  import Asterisk from "../lib/Asterisk.svelte";

  let {
    error = "",
    first,
    oncreated,
    oncancel,
  }: { error?: string; first: boolean; oncreated: (chain: Chain) => void; oncancel: () => void } = $props();

  let name = $state("");
  let saving = $state(false);
  let failure = $state("");

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    saving = true;
    failure = "";
    try {
      oncreated(await api.addChain(name));
    } catch (err) {
      failure = message(err);
    }
    saving = false;
  }
</script>

<svelte:window onkeydown={(event) => event.key === "Escape" && !first && oncancel()} />

<main class="welcome">
  <form class="card" onsubmit={submit}>
    <span class="mark"><Asterisk size={40} /></span>
    {#if first}
      <h1>Um SQL, todas as lojas.</h1>
      <p class="muted">Comece pela rede: a empresa cujas lojas você atende. Depois vêm os perfis de sistema e as lojas.</p>
    {:else}
      <h1>Nova rede</h1>
      <p class="muted">Cada rede tem suas próprias lojas, perfis e consultas.</p>
    {/if}
    <label for="chain-name">Nome da rede</label>
    <!-- svelte-ignore a11y_autofocus -->
    <input id="chain-name" class="input" bind:value={name} placeholder="Ex.: Supermercados Aurora" autofocus maxlength="80" />
    {#if failure || error}<p class="error" role="alert">{failure || error}</p>{/if}
    <div class="actions">
      {#if !first}<button type="button" class="btn" onclick={oncancel}>Cancelar</button>{/if}
      <button class="btn primary" disabled={saving || !name.trim()}>Criar rede</button>
    </div>
  </form>
</main>

<style>
  .welcome { height: 100%; display: grid; place-items: center; padding: 24px; }
  form { width: 440px; padding: 32px; display: grid; gap: 10px; }
  .mark { color: var(--brand); margin-bottom: 8px; }
  h1 { margin: 0; font-size: var(--text-2xl); font-weight: 600; letter-spacing: -0.015em; }
  p { margin: 0 0 12px; }
  label { font-weight: 500; font-size: var(--text-sm); }
  .error { color: var(--err); font-size: var(--text-sm); margin: 0; }
  .actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 8px; }
</style>
