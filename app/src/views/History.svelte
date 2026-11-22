<script lang="ts">
  import { api, message, when, type HistoryEntry, type HistoryUsage } from "../lib/api";
  import Asterisk from "../lib/Asterisk.svelte";
  import Modal from "../lib/Modal.svelte";

  let { chainId, onreopen }: { chainId: number; onreopen: (id: number) => void } = $props();

  let entries = $state<HistoryEntry[]>([]);
  let usage = $state<HistoryUsage | null>(null);
  let loaded = $state(false);
  let failure = $state("");
  let limitsOpen = $state(false);
  let days = $state(30);
  let megabytes = $state(500);
  let limitsFailure = $state("");

  $effect(() => {
    load(chainId);
  });

  async function load(id: number) {
    try {
      [entries, usage] = await Promise.all([api.history(id), api.historyUsage()]);
      failure = "";
    } catch (error) {
      failure = message(error);
    }
    loaded = true;
  }

  function size(bytes: number): string {
    const mb = bytes / (1024 * 1024);
    if (mb >= 1) return `${mb.toLocaleString("pt-BR", { maximumFractionDigits: 1 })} MB`;
    return `${Math.max(1, Math.round(bytes / 1024)).toLocaleString("pt-BR")} KB`;
  }

  function duration(ms: number): string {
    return `${(ms / 1000).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} s`;
  }

  function openLimits() {
    days = usage?.limits.days ?? 30;
    megabytes = usage?.limits.megabytes ?? 500;
    limitsFailure = "";
    limitsOpen = true;
  }

  async function saveLimits(event: SubmitEvent) {
    event.preventDefault();
    try {
      usage = await api.setHistoryLimits({ days, megabytes });
      entries = await api.history(chainId);
      limitsOpen = false;
    } catch (error) {
      limitsFailure = message(error);
    }
  }

  async function remove(entry: HistoryEntry) {
    try {
      await api.deleteHistory(entry.id);
      await load(chainId);
    } catch (error) {
      failure = message(error);
    }
  }

  function params(entry: HistoryEntry): string {
    return Object.entries(entry.values).map(([name, value]) => `${name} = ${value}`).join(" · ");
  }
</script>

<section class="history">
  <div class="head">
    <span class="muted">
      Leituras dos últimos {usage?.limits.days ?? 30} dias, com os dados retornados
      {#if usage} · {size(usage.bytes)} de {usage.limits.megabytes.toLocaleString("pt-BR")} MB em todas as redes{/if}
    </span>
    <span class="grow"></span>
    <button class="btn" onclick={openLimits}>Limites</button>
    <button class="btn" onclick={() => load(chainId)}>Atualizar</button>
  </div>
  {#if failure}<p class="error-text pad" role="alert">{failure}</p>{/if}
  {#if loaded && !entries.length && !failure}
    <div class="empty">
      <Asterisk size={40} />
      <h3>Nenhuma execução guardada</h3>
      <p>Cada leitura feita na Consulta fica aqui com o resultado de cada equipamento. Correções ficam na Auditoria.</p>
    </div>
  {:else if entries.length}
    <div class="table-wrap">
      <table>
        <thead><tr><th>Quando</th><th>Consulta</th><th class="num">Alvos</th><th>Resultado</th><th class="num">Linhas</th><th class="num">Duração</th><th class="num">Tamanho</th><th></th></tr></thead>
        <tbody>
          {#each entries as entry (entry.id)}
            <tr>
              <td class="num">{when(entry.at)}</td>
              <td class="query" title={entry.sql ?? "SQL não guardado: cita senha, token ou segredo"}>
                <span class:muted={entry.sql === null}>{entry.label}</span>
                {#if Object.keys(entry.values).length}<span class="values muted">{params(entry)}</span>{/if}
              </td>
              <td class="num">{entry.targets.toLocaleString("pt-BR")}</td>
              <td>
                <span class="pill ok">{entry.responded.toLocaleString("pt-BR")} responderam</span>
                {#if entry.offline}<span class="pill off">{entry.offline} offline</span>{/if}
                {#if entry.failed}<span class="pill err">{entry.failed} {entry.failed === 1 ? "falhou" : "falharam"}</span>{/if}
              </td>
              <td class="num">{entry.rows.toLocaleString("pt-BR")}</td>
              <td class="num">{duration(entry.elapsed_ms)}</td>
              <td class="num">{size(entry.bytes)}</td>
              <td class="actions">
                <button class="btn small ghost danger" onclick={() => remove(entry)} aria-label="Excluir execução de {when(entry.at)}">Excluir</button>
                <button class="btn small" onclick={() => onreopen(entry.id)}>Reabrir</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

{#if limitsOpen}
  <Modal title="Limites do histórico" subtitle="Ao passar de qualquer um, as execuções mais antigas saem. A mais recente sempre fica." width={460} onclose={() => (limitsOpen = false)}>
    <form id="limits-form" onsubmit={saveLimits}>
      <label class="field">
        <span class="label">Guardar por até (dias)</span>
        <input class="input" type="number" min="1" max="3650" bind:value={days} required />
      </label>
      <label class="field">
        <span class="label">Espaço máximo (MB)</span>
        <input class="input" type="number" min="1" max="100000" bind:value={megabytes} required />
      </label>
      {#if limitsFailure}<p class="error-text" role="alert">{limitsFailure}</p>{/if}
    </form>
    {#snippet footer()}
      <button class="btn" onclick={() => (limitsOpen = false)}>Cancelar</button>
      <button class="btn primary" form="limits-form">Salvar</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .history { flex: 1; display: flex; flex-direction: column; min-height: 0; padding: 0 20px 16px; }
  .head { display: flex; align-items: center; gap: 10px; padding: 14px 0; font-size: var(--text-sm); flex: none; }
  .grow { flex: 1; }
  .pad { padding: 0 0 10px; }
  .table-wrap { flex: 1; min-height: 0; }
  .query { max-width: 440px; overflow: hidden; text-overflow: ellipsis; }
  .values { margin-left: 8px; font-size: var(--text-xs); font-family: var(--font-mono); }
  td .pill + .pill { margin-left: 4px; }
  .actions { text-align: right; white-space: nowrap; }
  .empty { margin: auto; text-align: center; color: var(--border-strong); padding: 40px; display: grid; justify-items: center; gap: 6px; }
  .empty h3 { color: var(--text); font-size: var(--text-lg); font-weight: 600; margin: 4px 0 0; }
  .empty p { color: var(--text-muted); margin: 0; max-width: 52ch; }
</style>
