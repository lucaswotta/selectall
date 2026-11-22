<script lang="ts">
  import { api, fileStamp, message, stepLabel, type AuditDevice, type ExportFormat, type AuditEntry, type RunStatus } from "../lib/api";
  import EmptyState from "../lib/EmptyState.svelte";

  let { chainId }: { chainId: number } = $props();

  let entries = $state<AuditEntry[]>([]);
  /** Por padrão só a rede aberta, como Histórico e Salvas. */
  let allChains = $state(false);
  let failure = $state("");
  let loaded = $state(false);
  let openId = $state<number | null>(null);
  let devices = $state<AuditDevice[]>([]);
  let notice = $state("");

  const statusText: Record<RunStatus, [string, string]> = {
    rehearsed: ["desfeito no ensaio", "desfeitos no ensaio"],
    applied: ["aplicado", "aplicados"],
    unknown: ["com resultado desconhecido", "com resultado desconhecido"],
    failed: ["falhou", "falharam"],
    offline: ["offline", "offline"],
    cancelled: ["cancelado", "cancelados"],
    timed_out: ["com prazo esgotado", "com prazo esgotado"],
    success: ["respondeu", "responderam"],
    partial: ["parcial", "parciais"],
  };

  const deviceLabel: Record<RunStatus, string> = {
    rehearsed: "Ensaio desfeito",
    applied: "Aplicado",
    unknown: "Resultado desconhecido",
    failed: "Falhou",
    offline: "Offline",
    cancelled: "Cancelado",
    timed_out: "Prazo esgotado",
    success: "Respondeu",
    partial: "Parcial",
  };

  // Colunas que repetiriam o mesmo valor em todas as linhas ficam escondidas; a exportação leva todas.
  const showChain = $derived(allChains && new Set(entries.map((e) => e.chain)).size > 1);
  const showUser = $derived(new Set(entries.map((e) => e.user)).size > 1);
  const columns = $derived(7 + Number(showChain) + Number(showUser));

  $effect(() => {
    void allChains;
    void chainId;
    openId = null;
    load();
  });

  async function load() {
    try {
      entries = await api.auditLog(allChains ? null : chainId);
      failure = "";
    } catch (error) {
      failure = message(error);
    }
    loaded = true;
  }

  async function toggle(entry: AuditEntry) {
    if (openId === entry.id) {
      openId = null;
      return;
    }
    try {
      devices = await api.auditDevices(entry.id);
      openId = entry.id;
    } catch (error) {
      failure = message(error);
    }
  }

  async function exportAs(format: ExportFormat) {
    try {
      const path = await api.exportAudit(allChains ? null : chainId, format, `selectall-auditoria-${fileStamp()}`);
      notice = `Salvo em ${path}`;
      failure = "";
    } catch (error) {
      failure = message(error);
    }
  }

  function storesLabel(entry: AuditEntry): string {
    const { stores } = entry;
    if (!stores.length) return "–";
    if (stores.length <= 3) return stores.join(", ");
    return `${stores.slice(0, 2).join(", ")} e mais ${stores.length - 2}`;
  }

  function when(at: string): string {
    return new Date(at).toLocaleString("pt-BR", { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });
  }

  function outcome(entry: AuditEntry): string {
    const parts = entry.statuses.map(([status, count]) => `${count} ${statusText[status]?.[count === 1 ? 0 : 1] ?? status}`);
    if (entry.finished < entry.planned) parts.push(`${entry.planned - entry.finished} sem resultado`);
    return parts.join(" · ") || "Sem resultado";
  }

  function tone(entry: AuditEntry): string {
    const has = (...wanted: RunStatus[]) => entry.statuses.some(([status]) => wanted.includes(status));
    if (has("unknown")) return "warn";
    if (has("failed", "timed_out") || entry.finished < entry.planned) return "err";
    if (has("offline", "cancelled")) return "off";
    return entry.step === "rehearsal" ? "run" : "ok";
  }

  function deviceTone(status: RunStatus): string {
    if (status === "applied") return "ok";
    if (status === "rehearsed") return "run";
    if (status === "unknown") return "warn";
    if (status === "offline" || status === "cancelled") return "off";
    return "err";
  }
</script>

<section class="audit">
  <div class="head">
    {#if notice}
      <span class="muted selectable" role="status">{notice}</span>
    {:else}
      <span class="muted">Toda etapa de correção, com o resultado de cada equipamento. Registro local, somente acréscimo.</span>
    {/if}
    <span class="grow"></span>
    <div class="seg" role="group" aria-label="Redes mostradas">
      <button aria-pressed={!allChains} onclick={() => (allChains = false)}>Esta rede</button>
      <button aria-pressed={allChains} onclick={() => (allChains = true)}>Todas as redes</button>
    </div>
    <button class="btn" onclick={() => exportAs("csv")} disabled={!entries.length}>Exportar CSV</button>
    <button class="btn" onclick={() => exportAs("xlsx")} disabled={!entries.length}>Exportar XLSX</button>
    <button class="btn" onclick={load}>Atualizar</button>
  </div>
  {#if failure}<p class="error-text pad" role="alert">{failure}</p>{/if}
  {#if loaded && !entries.length && !failure}
    <EmptyState title={allChains ? "Nenhuma correção ainda" : "Nenhuma correção nesta rede"}>
      <p>Ensaios, pilotos e lotes feitos no modo Corrigir aparecem aqui, com o resultado de cada equipamento.</p>
    </EmptyState>
  {:else if entries.length}
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Quando</th><th>Etapa</th><th>SQL</th><th>Lojas</th><th class="num">Equipamentos</th><th class="num">Linhas</th><th>Resultado</th>
            {#if showChain}<th>Rede</th>{/if}
            {#if showUser}<th>Usuário</th>{/if}
          </tr>
        </thead>
        <tbody>
          {#each entries as entry (entry.id)}
            <tr class="entry" class:selected={openId === entry.id} onclick={() => toggle(entry)}>
              <td class="when">{when(entry.at)}</td>
              <td><button class="link" aria-expanded={openId === entry.id} onclick={(event) => { event.stopPropagation(); toggle(entry); }}>{stepLabel[entry.step]}</button>{#if entry.rehearsal_skipped}{" "}<span class="pill warn" title="Ensaio pulado por escolha de quem corrigiu">sem ensaio</span>{/if}</td>
              <td class="data sql" title={entry.sql}>{entry.sql}</td>
              <td class="mono" title={entry.stores.join(", ")}>{storesLabel(entry)}</td>
              <td class="num">{entry.finished < entry.planned ? `${entry.finished} de ${entry.planned}` : entry.planned}</td>
              <td class="num">{entry.affected.toLocaleString("pt-BR")}</td>
              <td><span class="pill {tone(entry)}">{outcome(entry)}</span></td>
              {#if showChain}<td>{entry.chain}</td>{/if}
              {#if showUser}<td>{entry.user}</td>{/if}
            </tr>
            {#if openId === entry.id}
              <tr class="detail">
                <td colspan={columns}>
                  <pre class="selectable">{entry.sql}</pre>
                  <table>
                    <thead><tr><th>Loja</th><th>Equipamento</th><th>IP</th><th>Perfil</th><th class="num">Linhas</th><th>Situação</th><th>Mensagem</th></tr></thead>
                    <tbody>
                      {#each devices as device, index (index)}
                        <tr>
                          <td class="mono">{device.store}</td>
                          <td>{device.device}</td>
                          <td class="data muted">{device.host}</td>
                          <td>{device.profile ?? ""}</td>
                          <td class="num">{device.affected.toLocaleString("pt-BR")}</td>
                          <td><span class="pill {deviceTone(device.status)}">{deviceLabel[device.status] ?? device.status}</span></td>
                          <td class="message selectable">{device.message ?? ""}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<style>
  .audit { flex: 1; display: flex; flex-direction: column; min-height: 0; padding: 0 20px 16px; }
  .head { display: flex; align-items: center; gap: 12px; padding: 14px 0; font-size: var(--text-sm); flex: none; }
  .grow { flex: 1; }
  .pad { padding: 0 0 10px; }
  .table-wrap { flex: 1; min-height: 0; }
  .entry { cursor: pointer; }
  .sql { max-width: 420px; overflow: hidden; text-overflow: ellipsis; }
  tr.detail > td { background: var(--bg); padding: 12px; white-space: normal; height: auto; }
  tr.detail:hover > td { background: var(--bg); }
  tr.detail pre { font: 12px/1.5 var(--font-mono); white-space: pre-wrap; margin: 0 0 10px; }
  tr.detail table { border: 1px solid var(--border); border-radius: var(--radius); }
  .message { white-space: normal; }
</style>
