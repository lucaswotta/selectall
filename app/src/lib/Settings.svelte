<script lang="ts">
  import { onMount } from "svelte";
  import { api, message, type HistoryUsage } from "./api";
  import Modal from "./Modal.svelte";

  let { onclose, onsaved }: { onclose: () => void; onsaved?: () => void } = $props();

  let perStore = $state(4);
  let days = $state(30);
  let megabytes = $state(500);
  let usage = $state<HistoryUsage | null>(null);
  let loaded = $state(false);
  let saving = $state(false);
  let failure = $state("");

  onMount(async () => {
    try {
      [perStore, usage] = await Promise.all([api.perStoreLimit(), api.historyUsage()]);
      days = usage.limits.days;
      megabytes = usage.limits.megabytes;
    } catch (error) {
      failure = message(error);
    }
    loaded = true;
  });

  function size(bytes: number): string {
    const mb = bytes / (1024 * 1024);
    return `${mb.toLocaleString("pt-BR", { maximumFractionDigits: 1 })} MB`;
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    saving = true;
    failure = "";
    try {
      await api.setPerStoreLimit(perStore);
      await api.setHistoryLimits({ days, megabytes });
      onsaved?.();
      onclose();
    } catch (error) {
      failure = message(error);
    }
    saving = false;
  }
</script>

<Modal title="Ajustes" subtitle="Valem para todas as redes deste computador." width={520} {onclose}>
  <form id="settings-form" onsubmit={save}>
    <h3 class="section-title">Execução</h3>
    <label class="field">
      <span class="label">Equipamentos da mesma loja ao mesmo tempo</span>
      <input class="input narrow" type="number" min="1" max="32" bind:value={perStore} required disabled={!loaded} aria-describedby="per-store-hint" />
      <span class="hint" id="per-store-hint">Lojas com link lento ficam mais estáveis com 2 a 4. No total, até 16 equipamentos rodam juntos.</span>
    </label>
    <h3 class="section-title">Histórico</h3>
    <div class="pair">
      <label class="field">
        <span class="label">Guardar por até (dias)</span>
        <input class="input" type="number" min="1" max="3650" bind:value={days} required disabled={!loaded} />
      </label>
      <label class="field">
        <span class="label">Espaço máximo (MB)</span>
        <input class="input" type="number" min="1" max="100000" bind:value={megabytes} required disabled={!loaded} />
      </label>
    </div>
    <p class="hint">
      Ao passar de qualquer limite, as execuções mais antigas saem; a mais recente sempre fica.
      {#if usage}Em uso: {size(usage.bytes)} em {usage.runs.toLocaleString("pt-BR")} {usage.runs === 1 ? "execução" : "execuções"}.{/if}
    </p>
    {#if failure}<p class="error-text" role="alert">{failure}</p>{/if}
  </form>
  {#snippet footer()}
    <button class="btn" onclick={onclose}>Cancelar</button>
    <button class="btn primary" form="settings-form" disabled={!loaded || saving}>{saving ? "Salvando…" : "Salvar"}</button>
  {/snippet}
</Modal>

<style>
  .section-title { margin: 0 0 10px; }
  .section-title:not(:first-child) { margin-top: 18px; }
  .narrow { width: 120px; }
  .pair { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
  .hint { font-size: var(--text-xs); color: var(--text-muted); margin: 0; }
</style>
