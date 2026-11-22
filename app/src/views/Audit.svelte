<script lang="ts">
  import { onMount } from "svelte";
  import { api, fileStamp, message, stepLabel, type AuditDevice, type ExportFormat, type AuditEntry, type RunStatus } from "../lib/api";
  import EmptyState from "../lib/EmptyState.svelte";

  let entries = $state<AuditEntry[]>([]);
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

  onMount(load);

  async function load() {
    try {
      entries = await api.auditLog();
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
      const path = await api.exportAudit(format, `selectall-auditoria-${fileStamp()}`);
      notice = `Salvo em ${path}`;
      failure = "";
    } catch (error) {
      failure = message(error);
    }
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
    <span class="muted">Toda etapa de correção, com o resultado de cada equipamento. Registro local, somente acréscimo.</span>
    <span class="grow"></span>
    <button class="btn" onclick={() => exportAs("csv")} disabled={!entries.length}>Exportar CSV</button>
    <button class="btn" onclick={() => exportAs("xlsx")} disabled={!entries.length}>Exportar XLSX</button>
    <button class="btn" onclick={load}>Atualizar</button>
  </div>
  {#if failure}<p class="error-text pad" role="alert">{failure}</p>{/if}
  {#if notice}<p class="muted pad" role="status">{notice}</p>{/if}
  {#if loaded && !entries.length && !failure}
    <EmptyState title="Nenhuma correção ainda">
      <p>Ensaios, pilotos e lotes feitos no modo Corrigir aparecem aqui.</p>
    </EmptyState>
  {:else if entries.length}
    <div class="table-wrap">
      <table>
        <thead><tr><th>Quando</th><th>Usuário</th><th>Rede</th><th>SQL</th><th>Etapa</th><th class="num">Equipamentos</th><th class="num">Linhas</th><th>Resultado</th></tr></thead>
        <tbody>
          {#each entries as entry (entry.id)}
            <tr class="entry" class:selected={openId === entry.id} onclick={() => toggle(entry)}>
              <td class="num">{when(entry.at)}</td>
              <td>{entry.user}</td>
              <td>{entry.chain}</td>
              <td class="data sql" title={entry.sql}>{entry.sql}</td>
              <td><button class="link" aria-expanded={openId === entry.id} onclick={(event) => { event.stopPropagation(); toggle(entry); }}>{stepLabel[entry.step]}</button>{#if entry.rehearsal_skipped} <span class="pill warn" title="Ensaio pulado por escolha de quem corrigiu">sem ensaio</span>{/if}</td>
              <td class="num">{entry.finished < entry.planned ? `${entry.finished} de ${entry.planned}` : entry.planned}</td>
              <td class="num">{entry.affected.toLocaleString("pt-BR")}</td>
              <td><span class="pill {tone(entry)}">{outcome(entry)}</span></td>
            </tr>
            {#if openId === entry.id}
              <tr class="detail">
                <td colspan="8">
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
