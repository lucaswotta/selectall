<script lang="ts">
  import { api, engineLabel, message, plural, type AddressRule, type Entry, type Fleet, type Plan, type ScanHit, type Source } from "../lib/api";
  import Modal from "../lib/Modal.svelte";
  import { untrack } from "svelte";

  type Tab = "regra" | "colar" | "planilha" | "varredura";

  let {
    chainId,
    fleet,
    onadded,
    onclose,
  }: { chainId: number; fleet: Fleet; onadded: (summary: string) => Promise<void>; onclose: () => void } = $props();

  const tabList: { id: Tab; label: string }[] = [
    { id: "regra", label: "Regra de rede" },
    { id: "colar", label: "Colar lista" },
    { id: "planilha", label: "Planilha" },
    { id: "varredura", label: "Varredura" },
  ];
  const PREVIEW_ROWS = 300;

  // A janela edita uma cópia da frota de quando foi aberta.
  const start = untrack(() => ({ rules: fleet.rules, stores: fleet.stores.length, port: fleet.profiles[0]?.port ?? 3306, profile: fleet.profiles[0]?.id ?? null }));
  const firstProfile = start.profile;
  const initialRules: AddressRule[] = start.rules.length
    ? start.rules.map((rule) => ({ ...rule }))
    : [
        { kind: "PDV", template: "10.{sub}.3.1-50", profile_id: firstProfile },
        { kind: "Self", template: "10.{sub}.3.101-130", profile_id: firstProfile },
        { kind: "Concentrador", template: "10.{sub}.3.200", profile_id: firstProfile },
      ];

  let tab = $state<Tab>(start.rules.length || start.stores === 0 ? "regra" : "colar");
  let rules = $state<AddressRule[]>(initialRules);
  let storeLines = $state("");
  let scanRule = $state(false);
  let pasted = $state("");
  let file = $state<{ name: string; bytes: number[] } | null>(null);
  let dragging = $state(false);
  let range = $state("");
  let port = $state(start.port);
  let scanStore = $state("");
  let scanKind = $state("");
  let scanProfile = $state<number | null>(firstProfile);
  let hits = $state<ScanHit[] | null>(null);
  let chosen = $state<Record<string, boolean>>({});

  let plan = $state<Plan | null>(null);
  let planKey = $state("");
  let busy = $state("");
  let failure = $state("");

  const source = $derived(buildSource());
  const sourceKey = $derived(source ? JSON.stringify(source, (key, value) => (key === "bytes" ? value.length : value)) : "");
  const fresh = $derived(plan !== null && planKey === sourceKey);
  const newDevices = $derived(plan ? plan.devices.filter((d) => d.status === "new").length : 0);
  const profileName = $derived(new Map(fleet.profiles.map((p) => [p.id, p.name])));

  function buildSource(): Source | null {
    switch (tab) {
      case "regra":
        return storeLines.trim() ? { kind: "rule", rules: $state.snapshot(rules), stores: storeLines, scan: scanRule } : null;
      case "colar":
        return pasted.trim() ? { kind: "paste", text: pasted } : null;
      case "planilha":
        return file ? { kind: "file", name: file.name, bytes: file.bytes } : null;
      case "varredura": {
        const entries: Entry[] = (hits ?? [])
          .filter((hit) => hit.open && chosen[hit.host])
          .map((hit, index) => ({
            line: index + 1,
            store_code: scanStore.trim() || null,
            host: hit.host,
            kind: scanKind.trim() || null,
            name: null,
            profile_id: scanProfile,
            profile_name: null,
          }));
        return entries.length ? { kind: "batch", batch: { stores: [], entries, errors: [], notices: [] } } : null;
      }
    }
  }

  function select(next: Tab) {
    tab = next;
    plan = null;
    failure = "";
  }

  async function preview() {
    if (!source) return;
    busy = scanRule && tab === "regra" ? "Varrendo as faixas…" : "Gerando prévia…";
    failure = "";
    const key = sourceKey;
    try {
      plan = await api.preview(chainId, source);
      planKey = key;
    } catch (error) {
      failure = message(error);
      plan = null;
    }
    busy = "";
  }

  async function add() {
    if (!plan || !fresh) return;
    busy = "Gravando…";
    failure = "";
    try {
      if (tab === "regra") await api.saveRules(chainId, $state.snapshot(rules));
      const applied = await api.apply(chainId, $state.snapshot(plan));
      await onadded(`${plural(applied.stores, "loja", "lojas")} e ${plural(applied.devices, "equipamento", "equipamentos")} adicionados`);
    } catch (error) {
      failure = message(error);
    }
    busy = "";
  }

  async function readFile(picked: File | undefined) {
    if (!picked) return;
    failure = "";
    plan = null;
    file = { name: picked.name, bytes: Array.from(new Uint8Array(await picked.arrayBuffer())) };
  }

  function drop(event: DragEvent) {
    event.preventDefault();
    dragging = false;
    readFile(event.dataTransfer?.files[0]);
  }

  async function runScan() {
    busy = "Varrendo…";
    failure = "";
    hits = null;
    plan = null;
    try {
      const found = await api.scanRange(range, port);
      hits = found.filter((hit) => hit.open);
      chosen = Object.fromEntries(hits.map((hit) => [hit.host, true]));
      if (!hits.length) failure = `Nenhum endereço respondeu na porta ${port}.`;
    } catch (error) {
      failure = message(error);
    }
    busy = "";
  }

  function versions(found: ScanHit[]): string {
    const counts = new Map<string, number>();
    for (const hit of found) {
      const version = hit.version ? ` ${hit.version.split(/[.-]/).slice(0, 2).join(".")}` : "";
      const label = hit.engine ? `${engineLabel[hit.engine]}${version}` : "motor não identificado";
      counts.set(label, (counts.get(label) ?? 0) + 1);
    }
    return [...counts].map(([label, count]) => `${count} ${label}`).join(", ");
  }

  const statusLabel = { new: ["Novo", "ok"], exists: ["Já cadastrado", "off"], repeated: ["Repetido", "warn"] } as const;
</script>

<Modal title="Adicionar lojas" subtitle="Escolha o caminho mais rápido. Tudo passa por uma prévia antes de entrar na frota." width={780} {onclose}>
  {#snippet tabs()}
    <div class="tabs" role="tablist">
      {#each tabList as item (item.id)}
        <button class="tab" role="tab" aria-selected={tab === item.id} onclick={() => select(item.id)}>{item.label}</button>
      {/each}
    </div>
  {/snippet}

  {#if tab === "regra"}
    <p class="muted intro">Defina uma vez como os equipamentos são numerados. Use <span class="mono">{"{sub}"}</span> para a sub-rede e <span class="mono">{"{loja}"}</span> para o código; a faixa vai no último número.</p>
    <div class="rule head"><span>Tipo</span><span>Endereço</span><span>Perfil</span><span></span></div>
    {#each rules as rule, index (index)}
      <div class="rule">
        <input class="input" bind:value={rule.kind} aria-label="Tipo" placeholder="PDV" />
        <input class="input mono" bind:value={rule.template} aria-label="Endereço" placeholder={"10.{sub}.3.1-50"} />
        <select class="input" bind:value={rule.profile_id} aria-label="Perfil">
          <option value={null}>Sem perfil</option>
          {#each fleet.profiles as profile (profile.id)}<option value={profile.id}>{profile.name}</option>{/each}
        </select>
        <button class="btn ghost small" aria-label="Remover linha" onclick={() => rules.splice(index, 1)}>Remover</button>
      </div>
    {/each}
    <button class="btn small" onclick={() => rules.push({ kind: "", template: "", profile_id: firstProfile })}>Adicionar tipo</button>
    <div class="field top">
      <label for="store-lines">Lojas e sub-redes <span class="hint">· uma por linha: código;nome;sub-rede (sem sub-rede, vale o código)</span></label>
      <textarea id="store-lines" class="input" bind:value={storeLines} placeholder={"101;Centro;101\n102;Jardim América;102"}></textarea>
    </div>
    <label class="check"><input type="checkbox" bind:checked={scanRule} />Varrer as faixas e manter só quem responder na porta do perfil</label>
  {:else if tab === "colar"}
    <div class="field">
      <label for="pasted">Cole IPs, faixas ou linhas <span class="mono">loja;ip;tipo;nome</span></label>
      <textarea id="pasted" class="input" bind:value={pasted} placeholder={"10.113.3.1-12\n10.113.3.200\n114;10.114.3.5;PDV"}></textarea>
    </div>
    <p class="muted intro">IP sem loja usa a regra da rede para achar a loja e o tipo. Sem tipo nem regra, o equipamento entra como PDV.</p>
  {:else if tab === "planilha"}
    <div
      class="card drop"
      class:dragging
      role="region"
      aria-label="Área para soltar a planilha"
      ondragover={(event) => (event.preventDefault(), (dragging = true))}
      ondragleave={() => (dragging = false)}
      ondrop={drop}
    >
      <strong>{file ? file.name : "Solte um arquivo CSV ou XLSX"}</strong>
      <div class="muted small-text">Colunas reconhecidas: loja, nome, sub-rede, ip, equipamento, tipo, perfil. Senhas não são lidas de planilhas.</div>
      <label class="btn pick">
        {file ? "Escolher outro arquivo" : "Escolher arquivo"}
        <input type="file" accept=".csv,.txt,.xlsx,.xlsm,.xls,.ods" onchange={(event) => readFile(event.currentTarget.files?.[0])} hidden />
      </label>
    </div>
  {:else}
    <div class="scan-grid">
      <div class="field">
        <label for="range">Faixa a varrer</label>
        <input id="range" class="input mono" bind:value={range} placeholder="10.115.3.1-254" />
      </div>
      <div class="field">
        <label for="port">Porta</label>
        <input id="port" class="input mono" type="number" min="1" max="65535" bind:value={port} />
      </div>
      <div class="field end"><button class="btn" disabled={!range.trim() || !!busy} onclick={runScan}>Varrer</button></div>
    </div>
    {#if hits?.length}
      <div class="preview-box"><span class="pill ok">Concluída</span> {plural(hits.length, "endereço responde", "endereços respondem")}: {versions(hits)}.</div>
      <div class="scan-grid three">
        <div class="field">
          <label for="scan-store">Loja <span class="hint">vazio: pela regra</span></label>
          <input id="scan-store" class="input mono" bind:value={scanStore} placeholder="115" />
        </div>
        <div class="field">
          <label for="scan-kind">Tipo <span class="hint">vazio: pela regra</span></label>
          <input id="scan-kind" class="input" bind:value={scanKind} placeholder="PDV" />
        </div>
        <div class="field">
          <label for="scan-profile">Perfil</label>
          <select id="scan-profile" class="input" bind:value={scanProfile}>
            <option value={null}>Pela regra</option>
            {#each fleet.profiles as profile (profile.id)}<option value={profile.id}>{profile.name}</option>{/each}
          </select>
        </div>
      </div>
      <div class="table-wrap hits">
        <table>
          <thead><tr><th></th><th>IP</th><th>Versão</th></tr></thead>
          <tbody>
            {#each hits as hit (hit.host)}
              <tr>
                <td><input type="checkbox" bind:checked={chosen[hit.host]} aria-label="Incluir {hit.host}" /></td>
                <td class="data">{hit.host}</td>
                <td class="muted">{hit.version ?? "–"}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}

  {#if failure}<p class="error-text gap" role="alert">{failure}</p>{/if}

  {#if plan}
    <div class="preview-box" class:stale={!fresh}>
      {#if !fresh}<strong>Os dados mudaram; gere a prévia de novo.</strong><br />{/if}
      Prévia: <strong>{plural(plan.stores.length, "loja nova", "lojas novas")}</strong>, <strong>{plural(newDevices, "equipamento novo", "equipamentos novos")}</strong>{#if plan.devices.length > newDevices}, {plural(plan.devices.length - newDevices, "ignorado por duplicidade", "ignorados por duplicidade")}{/if}{#if plan.errors.length}, <span class="err-inline">{plural(plan.errors.length, "linha com erro", "linhas com erro")}</span>{/if}.
      {#each plan.notices as notice (notice)}<div class="muted">{notice}</div>{/each}
    </div>
    {#if plan.errors.length}
      <ul class="errors">
        {#each plan.errors.slice(0, 50) as error, index (index)}<li><span class="mono">Linha {error.line}</span> {error.message}</li>{/each}
        {#if plan.errors.length > 50}<li class="muted">e mais {plan.errors.length - 50}</li>{/if}
      </ul>
    {/if}
    {#if plan.devices.length}
      <div class="table-wrap planned">
        <table>
          <thead><tr><th>Loja</th><th>Equipamento</th><th>Tipo</th><th>IP</th><th>Perfil</th><th>Situação</th></tr></thead>
          <tbody>
            {#each plan.devices.slice(0, PREVIEW_ROWS) as device, index (index)}
              <tr>
                <td class="mono">{device.store_code}</td>
                <td>{device.name}</td>
                <td>{device.kind}</td>
                <td class="data">{device.host}</td>
                <td>{device.profile_ids.map((id) => profileName.get(id) ?? "?").join(", ") || "–"}</td>
                <td><span class="pill {statusLabel[device.status][1]}">{statusLabel[device.status][0]}</span></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if plan.devices.length > PREVIEW_ROWS}<p class="muted small-text">Mostrando {PREVIEW_ROWS} de {plan.devices.length.toLocaleString("pt-BR")} linhas.</p>{/if}
    {/if}
  {/if}

  {#snippet footer()}
    <span class="grow muted small-text">{busy}</span>
    <button class="btn" onclick={onclose}>Cancelar</button>
    {#if fresh && plan}
      <button class="btn primary" disabled={!!busy || (newDevices === 0 && plan.stores.length === 0)} onclick={add}>
        Adicionar {plural(plan.stores.length, "loja", "lojas")} e {plural(newDevices, "equipamento", "equipamentos")}
      </button>
    {:else}
      <button class="btn primary" disabled={!source || !!busy} onclick={preview}>Gerar prévia</button>
    {/if}
  {/snippet}
</Modal>

<style>
  .intro { margin: 0 0 12px; font-size: var(--text-sm); }
  .rule { display: grid; grid-template-columns: 130px 1fr 150px 70px; gap: 8px; align-items: center; margin-bottom: 8px; font-size: var(--text-sm); }
  .rule.head { font-weight: 600; margin-bottom: 4px; }
  .top { margin-top: 14px; }
  .drop { padding: 28px; text-align: center; border-style: dashed; display: grid; gap: 4px; justify-items: center; }
  .drop.dragging { border-color: var(--brand); background: var(--brand-soft); }
  .pick { margin-top: 10px; display: inline-block; }
  .small-text { font-size: var(--text-sm); }
  .scan-grid { display: grid; grid-template-columns: 1fr 120px auto; gap: 0 12px; }
  .scan-grid.three { grid-template-columns: 1fr 1fr 1fr; margin-top: 12px; }
  .end { align-content: end; }
  .hits { max-height: 180px; }
  .planned { max-height: 260px; margin-top: 10px; }
  .gap { margin-top: 12px; }
  .stale { outline: 1px solid var(--warn); }
  .err-inline { color: var(--err); }
  .errors { margin: 10px 0 0; padding: 0; list-style: none; font-size: var(--text-sm); display: grid; gap: 2px; max-height: 140px; overflow: auto; color: var(--err); }
  .errors .mono { color: var(--text-muted); margin-right: 6px; }
  .grow { flex: 1; }
</style>
