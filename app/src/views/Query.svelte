<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import {
    api,
    message,
    plural,
    type Device,
    type ExportFormat,
    type Failure,
    type Fleet,
    type Grid,
    type ProgressEvent,
    type RunStatus,
  } from "../lib/api";
  import Asterisk from "../lib/Asterisk.svelte";

  let { chainId, active }: { chainId: number; active: boolean } = $props();

  type Phase = "queued" | "connecting" | "running" | "skipped" | RunStatus;
  type DeviceState = {
    phase: Phase;
    profile: string | null;
    failure: Failure | null;
    rows: number;
    truncated: boolean;
    warnings: number;
  };
  type Item = { group: string; count: number } | { row: (string | null)[] };

  const ROW_HEIGHT = 30;
  const SQL_KEY = "selectall.sql";

  let fleet = $state<Fleet>({ stores: [], devices: [], profiles: [], rules: [] });
  let loadFailure = $state("");
  let firstLoad = true;
  const chosenStores = new SvelteSet<number>();
  const hiddenKinds = new SvelteSet<string>();
  let search = $state("");
  let sql = $state(readSql());
  let lineBox = $state<HTMLDivElement>();

  let runId = $state(0);
  let starting = false;
  let early: ProgressEvent[] = [];
  let running = $state(false);
  let cancelling = $state(false);
  let runFailure = $state("");
  let runDevices = $state<Device[]>([]);
  let states = $state<Record<number, DeviceState>>({});
  let startedAt = $state(0);
  let elapsed = $state(0);
  let finishedAt = $state<Date | null>(null);
  let log = $state<{ at: Date; text: string }[]>([]);

  let grid = $state<Grid | null>(null);
  let gridSet = $state(0);
  let tab = $state<"grade" | "falhas" | "msgs">("grade");
  let filter = $state("");
  let sortColumn = $state<number | null>(null);
  let sortDescending = $state(false);
  let grouped = $state(true);
  let notice = $state("");
  let scrollTop = $state(0);
  let viewport = $state(400);

  let unlisten: UnlistenFn | undefined;

  const kinds = $derived([...new Set(fleet.devices.map((d) => d.kind))].sort((a, b) => a.localeCompare(b, "pt-BR")));
  const storeById = $derived(new Map(fleet.stores.map((s) => [s.id, s])));
  const storeByCode = $derived(new Map(fleet.stores.map((s) => [s.code, s])));
  const profileById = $derived(new Map(fleet.profiles.map((p) => [p.id, p])));
  const visibleStores = $derived(
    fleet.stores.filter((s) => {
      const term = search.trim().toLowerCase();
      return !term || s.code.toLowerCase().includes(term) || s.name.toLowerCase().includes(term);
    }),
  );
  const targets = $derived(fleet.devices.filter((d) => chosenStores.has(d.store_id) && !hiddenKinds.has(d.kind)));
  const chosenCount = $derived(new Set(targets.map((d) => d.store_id)).size);
  const profileSummary = $derived(summarizeProfiles(targets));
  const lineCount = $derived(sql.split("\n").length);

  const finished = $derived(runDevices.map((d) => [d, states[d.id]] as const).filter(([, s]) => s && isFinal(s.phase)));
  const okDevices = $derived(finished.filter(([, s]) => s.phase === "success" || s.phase === "partial"));
  const offlineDevices = $derived(finished.filter(([, s]) => s.phase === "offline" || s.phase === "skipped" || s.phase === "cancelled"));
  const failedDevices = $derived(finished.filter(([, s]) => s.phase === "failed" || s.phase === "timed_out" || (s.phase === "partial" && s.failure)));
  const totalRows = $derived(okDevices.reduce((sum, [, s]) => sum + s.rows, 0));
  const tiles = $derived(storeTiles());

  const items = $derived(buildItems());
  const firstItem = $derived(Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - 10));
  const lastItem = $derived(Math.min(items.length, firstItem + Math.ceil(viewport / ROW_HEIGHT) + 20));

  $effect(() => {
    if (active) load(chainId);
  });

  onMount(async () => {
    unlisten = await listen<ProgressEvent>("query-progress", (event) => receive(event.payload));
  });

  onDestroy(() => unlisten?.());

  function readSql(): string {
    try {
      return localStorage.getItem(SQL_KEY) ?? "SELECT VERSION() AS versao;";
    } catch {
      return "SELECT VERSION() AS versao;";
    }
  }

  function rememberSql() {
    try {
      localStorage.setItem(SQL_KEY, sql);
    } catch {
      // Sem armazenamento local o SQL só não volta na próxima abertura.
    }
  }

  async function load(id: number) {
    try {
      fleet = await api.fleet(id);
      loadFailure = "";
      for (const storeId of [...chosenStores]) if (!fleet.stores.some((s) => s.id === storeId)) chosenStores.delete(storeId);
      if (firstLoad) {
        for (const store of fleet.stores) chosenStores.add(store.id);
        firstLoad = false;
      }
    } catch (error) {
      loadFailure = message(error);
    }
  }

  function summarizeProfiles(devices: Device[]): { main: string[]; fallback: string[]; missing: string[] } {
    const main = new Set<string>();
    const fallback = new Set<string>();
    const missing = new Set<string>();
    for (const device of devices) {
      device.profile_ids.forEach((id, index) => {
        const profile = profileById.get(id);
        if (!profile) return;
        (index === 0 ? main : fallback).add(profile.name);
        if (!profile.has_password) missing.add(profile.name);
      });
    }
    for (const name of main) fallback.delete(name);
    return { main: [...main], fallback: [...fallback], missing: [...missing] };
  }

  function toggleKind(kind: string) {
    if (hiddenKinds.has(kind)) hiddenKinds.delete(kind);
    else hiddenKinds.add(kind);
  }

  function onlyKind(kind: string) {
    hiddenKinds.clear();
    for (const other of kinds) if (other !== kind) hiddenKinds.add(other);
  }

  function toggleStore(id: number) {
    if (chosenStores.has(id)) chosenStores.delete(id);
    else chosenStores.add(id);
  }

  function countFor(storeId: number): number {
    return fleet.devices.filter((d) => d.store_id === storeId && !hiddenKinds.has(d.kind)).length;
  }

  function isFinal(phase: Phase): boolean {
    return phase !== "queued" && phase !== "connecting" && phase !== "running";
  }

  function blank(): DeviceState {
    return { phase: "queued", profile: null, failure: null, rows: 0, truncated: false, warnings: 0 };
  }

  function note(text: string) {
    log.push({ at: new Date(), text });
  }

  function label(device: Device): string {
    const store = storeById.get(device.store_id);
    return `${store?.code ?? "?"} · ${device.name}`;
  }

  async function run() {
    if (running || !targets.length || !sql.trim()) return;
    rememberSql();
    const devices = [...targets];
    const ids = devices.map((d) => d.id);
    await begin(() => api.runQuery(chainId, ids, sql), () => {
      runDevices = devices;
      states = Object.fromEntries(ids.map((id) => [id, blank()]));
      log = [];
      grid = null;
      gridSet = 0;
      sortColumn = null;
      note(`Execução iniciada em ${plural(devices.length, "equipamento", "equipamentos")} de ${plural(new Set(devices.map((d) => d.store_id)).size, "loja", "lojas")}, até 16 por vez.`);
      note("Modo somente leitura: transação READ ONLY exigida pelo servidor.");
    });
  }

  async function retry() {
    const devices = [...failedDevices, ...offlineDevices].map(([d]) => d);
    if (running || !devices.length) return;
    const ids = devices.map((d) => d.id);
    await begin(() => api.retryQuery(chainId, ids), () => {
      for (const id of ids) states[id] = blank();
      note(`Repetindo em ${plural(devices.length, "equipamento", "equipamentos")}; os demais resultados foram mantidos.`);
    });
    tab = "grade";
  }

  async function begin(call: () => Promise<number>, reset: () => void) {
    runFailure = "";
    notice = "";
    starting = true;
    early = [];
    try {
      const id = await call();
      reset();
      runId = id;
      running = true;
      cancelling = false;
      startedAt = performance.now();
      finishedAt = null;
      const replay = early.filter((event) => event.run === id);
      early = [];
      replay.forEach(receive);
    } catch (error) {
      runFailure = message(error);
    } finally {
      starting = false;
    }
  }

  function receive(event: ProgressEvent) {
    if (event.run !== runId) {
      // O comando devolve o número depois que os primeiros eventos já saíram.
      if (starting) early.push(event);
      return;
    }
    switch (event.type) {
      case "connecting":
        states[event.device_id] = { ...blank(), phase: "connecting" };
        break;
      case "running":
        states[event.device_id] = { ...blank(), phase: "running", profile: event.profile };
        break;
      case "finished": {
        states[event.device_id] = {
          phase: event.status,
          profile: event.profile,
          failure: event.failure,
          rows: event.rows,
          truncated: event.truncated,
          warnings: event.warnings,
        };
        const device = runDevices.find((d) => d.id === event.device_id);
        if (device) describeOutcome(device, states[event.device_id]);
        break;
      }
      case "skipped":
        states[event.device_id] = { ...blank(), phase: "skipped", failure: { message: "Não iniciado: a consulta foi cancelada.", detail: null } };
        break;
      case "cancel_errors":
        for (const error of event.errors) note(`O cancelamento não chegou a um equipamento: ${error}`);
        break;
      case "done":
        finish();
        break;
    }
  }

  function describeOutcome(device: Device, state: DeviceState) {
    const first = profileById.get(device.profile_ids[0])?.name;
    if (state.profile && first && state.profile !== first) note(`${label(device)} recusou ${first} e respondeu com ${state.profile}.`);
    if (state.truncated) note(`${label(device)}: resultado cortado no limite de 1.000 linhas.`);
    if (state.warnings) note(`${label(device)}: ${plural(state.warnings, "aviso", "avisos")} do servidor.`);
  }

  async function finish() {
    running = false;
    cancelling = false;
    elapsed = (performance.now() - startedAt) / 1000;
    finishedAt = new Date();
    if (offlineDevices.length) note(`${plural(offlineDevices.length, "equipamento ficou", "equipamentos ficaram")} sem resposta ou fora da execução.`);
    if (failedDevices.length) note(`${plural(failedDevices.length, "equipamento falhou", "equipamentos falharam")}; veja Offline e falhas.`);
    note(`Concluído em ${seconds(elapsed)}.`);
    await loadGrid(gridSet);
  }

  async function loadGrid(set: number) {
    try {
      grid = await api.grid(set);
      gridSet = set;
      sortColumn = null;
      scrollTop = 0;
    } catch (error) {
      runFailure = message(error);
    }
  }

  async function cancel() {
    cancelling = true;
    note("Cancelamento pedido: nenhum equipamento novo será iniciado.");
    await api.cancelQuery();
  }

  function seconds(value: number): string {
    return `${value.toLocaleString("pt-BR", { maximumFractionDigits: 1 })} s`;
  }

  function clock(date: Date): string {
    return date.toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  }

  function storeTiles() {
    const byStore = new Map<number, Device[]>();
    for (const device of runDevices) {
      const list = byStore.get(device.store_id);
      if (list) list.push(device);
      else byStore.set(device.store_id, [device]);
    }
    return [...byStore].map(([storeId, devices]) => {
      const phases = devices.map((d) => states[d.id]?.phase ?? "queued");
      const count = (...wanted: Phase[]) => phases.filter((p) => wanted.includes(p)).length;
      const total = devices.length;
      return {
        code: storeById.get(storeId)?.code ?? "?",
        name: storeById.get(storeId)?.name ?? "",
        total,
        done: phases.filter(isFinal).length,
        ok: (count("success", "partial") / total) * 100,
        err: (count("failed", "timed_out") / total) * 100,
        off: (count("offline", "skipped", "cancelled") / total) * 100,
      };
    });
  }

  function compareCells(a: string | null, b: string | null, numeric: boolean): number {
    if (a === b) return 0;
    if (a === null) return 1;
    if (b === null) return -1;
    if (numeric) return Number(a) - Number(b);
    return a.localeCompare(b, "pt-BR", { numeric: true });
  }

  function buildItems(): Item[] {
    if (!grid) return [];
    const term = filter.trim().toLowerCase();
    let rows = term ? grid.rows.filter((row) => row.some((cell) => cell?.toLowerCase().includes(term))) : grid.rows;
    if (sortColumn !== null) {
      const column = sortColumn;
      const numeric = grid.columns[column].numeric;
      rows = [...rows].sort((a, b) => compareCells(a[column], b[column], numeric) * (sortDescending ? -1 : 1));
    }
    if (!grouped || sortColumn !== null) return rows.map((row) => ({ row }));
    const result: Item[] = [];
    let current: string | null | undefined;
    let header: { group: string; count: number } | null = null;
    for (const row of rows) {
      if (row[0] !== current) {
        current = row[0];
        const store = current ? storeByCode.get(current) : undefined;
        header = { group: store ? `${store.code} · ${store.name}` : (current ?? "Sem loja"), count: 0 };
        result.push(header);
      }
      if (header) header.count += 1;
      result.push({ row });
    }
    return result;
  }

  function sortBy(column: number) {
    if (sortColumn === column) {
      if (sortDescending) sortColumn = null;
      sortDescending = !sortDescending;
    } else {
      sortColumn = column;
      sortDescending = false;
    }
  }

  function visibleRows(): (string | null)[][] {
    return items.flatMap((item) => ("row" in item ? [item.row] : []));
  }

  async function copy() {
    if (!grid) return;
    const cell = (value: string | null) => (value ?? "").replace(/[\t\n\r]/g, " ");
    const lines = [grid.columns.map((c) => c.name), ...visibleRows()].map((row) => row.map(cell).join("\t"));
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      notice = `${plural(lines.length - 1, "linha copiada", "linhas copiadas")} para a área de transferência.`;
    } catch (error) {
      notice = `Não foi possível copiar: ${message(error)}`;
    }
  }

  async function exportAs(format: ExportFormat) {
    const now = new Date();
    const pad = (n: number) => String(n).padStart(2, "0");
    const stem = `selectall-${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
    try {
      const path = await api.exportGrid(gridSet, format, stem);
      notice = `Salvo em ${path}`;
    } catch (error) {
      notice = message(error);
    }
  }

  function onEditorKey(event: KeyboardEvent) {
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      run();
    }
  }

  function phaseLabel(phase: Phase): string {
    return {
      queued: "Na fila",
      connecting: "Conectando",
      running: "Executando",
      skipped: "Não iniciado",
      success: "Respondeu",
      partial: "Parcial",
      failed: "Falhou",
      offline: "Offline",
      cancelled: "Cancelado",
      timed_out: "Prazo esgotado",
    }[phase];
  }

  function phaseClass(phase: Phase): string {
    if (phase === "failed" || phase === "timed_out") return "err";
    if (phase === "partial") return "warn";
    if (phase === "success") return "ok";
    return "off";
  }
</script>

<section class="query" class:gone={!active}>
  <aside class="targets" aria-label="Alvos">
    <header>
      <h2>Alvos</h2>
      <div class="muted small">{plural(targets.length, "equipamento", "equipamentos")} em {plural(chosenCount, "loja", "lojas")}</div>
    </header>
    {#if kinds.length}
      <div class="chips" role="group" aria-label="Tipos de equipamento">
        {#each kinds as kind (kind)}
          <button class="chip" aria-pressed={!hiddenKinds.has(kind)} onclick={() => toggleKind(kind)} ondblclick={() => onlyKind(kind)} title="Clique duplo: só {kind}">{kind}</button>
        {/each}
      </div>
    {/if}
    <div class="search"><input class="input" bind:value={search} placeholder="Buscar loja por código ou nome" aria-label="Buscar loja" /></div>
    <div class="store-list">
      {#each visibleStores as store (store.id)}
        <label class="store">
          <input type="checkbox" checked={chosenStores.has(store.id)} onchange={() => toggleStore(store.id)} />
          <span class="code">{store.code}</span>
          <span class="name">{store.name}</span>
          <span class="count">{countFor(store.id)}</span>
        </label>
      {:else}
        <p class="muted small pad">{fleet.stores.length ? "Nenhuma loja com esse termo." : "Cadastre lojas na Frota para consultar."}</p>
      {/each}
    </div>
    <footer>
      <button class="btn ghost" onclick={() => visibleStores.forEach((s) => chosenStores.add(s.id))}>Todas</button>
      <button class="btn ghost" onclick={() => visibleStores.forEach((s) => chosenStores.delete(s.id))}>Nenhuma</button>
    </footer>
  </aside>

  <div class="work">
    <div class="editor-area">
      <div class="editor-head">
        <div class="seg" role="group" aria-label="Modo">
          <button aria-pressed="true">Consultar</button>
          <button aria-pressed="false" disabled title="Correções chegam na próxima versão">Corrigir</button>
        </div>
        <span class="muted small">Somente leitura: o servidor recusa qualquer alteração.</span>
      </div>
      <div class="editor">
        <div class="lines" bind:this={lineBox} aria-hidden="true">
          {#each { length: lineCount }, index (index)}<div>{index + 1}</div>{/each}
        </div>
        <textarea
          bind:value={sql}
          spellcheck="false"
          aria-label="SQL"
          onkeydown={onEditorKey}
          onscroll={(event) => lineBox && (lineBox.scrollTop = event.currentTarget.scrollTop)}
        ></textarea>
      </div>
      <div class="runbar">
        {#if profileSummary.main.length}
          <span class="muted">
            Perfis: <strong>{profileSummary.main.join(", ")}</strong>{#if profileSummary.fallback.length}, alternativa {profileSummary.fallback.join(", ")}{/if}
          </span>
        {/if}
        {#if profileSummary.missing.length}
          <span class="pill warn" title="Cadastre a senha em Frota > Perfis de sistema">Sem senha no cofre: {profileSummary.missing.join(", ")}</span>
        {/if}
        {#if runFailure}<span class="error-text" role="alert">{runFailure}</span>{/if}
        {#if loadFailure}<span class="error-text" role="alert">{loadFailure}</span>{/if}
        <span class="grow"></span>
        <span class="muted"><span class="kbd">Ctrl</span> + <span class="kbd">Enter</span></span>
        {#if running}
          <button class="btn lg" onclick={cancel} disabled={cancelling}>{cancelling ? "Cancelando" : "Cancelar"}</button>
        {:else}
          <button class="btn primary lg" onclick={run} disabled={!targets.length || !sql.trim()}>Executar em {targets.length.toLocaleString("pt-BR")}</button>
        {/if}
      </div>
    </div>

    <div class="results">
      {#if !runDevices.length}
        <div class="empty">
          <Asterisk size={40} />
          <h3>Escolha lojas e execute</h3>
          <p>O resultado de todos os equipamentos aparece numa grade só, com loja e equipamento em cada linha.</p>
        </div>
      {:else}
        <div class="progress" aria-label="Progresso por loja">
          {#each tiles as tile (tile.code)}
            <div class="tile" title="{tile.code} · {tile.name}">
              <div class="t"><span class="mono">{tile.code}</span><span class="muted">{tile.done}/{tile.total}</span></div>
              <div class="bar">
                <span style="width:{tile.ok}%;background:var(--ok)"></span>
                <span style="width:{tile.err}%;background:var(--err)"></span>
                <span style="width:{tile.off}%;background:var(--off)"></span>
              </div>
            </div>
          {/each}
        </div>
        <div class="summary">
          {#if running}
            <span class="pill run">Executando</span>
            <span class="muted">{finished.length.toLocaleString("pt-BR")} de {runDevices.length.toLocaleString("pt-BR")}</span>
          {:else}
            <span class="pill ok">{okDevices.length.toLocaleString("pt-BR")} responderam</span>
            <span class="pill off">{offlineDevices.length.toLocaleString("pt-BR")} offline</span>
            {#if failedDevices.length}<span class="pill err">{failedDevices.length} {failedDevices.length === 1 ? "falhou" : "falharam"}</span>{/if}
            <span class="muted">{plural(totalRows, "linha", "linhas")} · {seconds(elapsed)}{#if finishedAt} · executado às {clock(finishedAt)}{/if}</span>
          {/if}
        </div>
        <div class="tabs" role="tablist">
          <button class="tab" role="tab" aria-selected={tab === "grade"} onclick={() => (tab = "grade")}>Resultado<span class="n">{(grid?.rows.length ?? 0).toLocaleString("pt-BR")}</span></button>
          <button class="tab" role="tab" aria-selected={tab === "falhas"} onclick={() => (tab = "falhas")}>Offline e falhas<span class="n">{failedDevices.length + offlineDevices.length}</span></button>
          <button class="tab" role="tab" aria-selected={tab === "msgs"} onclick={() => (tab = "msgs")}>Mensagens<span class="n">{log.length}</span></button>
        </div>

        {#if tab === "grade"}
          {#if !grid}
            <div class="empty"><p>{running ? "A grade aparece quando todos os equipamentos terminarem." : "Sem resultado."}</p></div>
          {:else}
            <div class="toolbar">
              <input class="input filter" bind:value={filter} placeholder="Filtrar linhas" aria-label="Filtrar linhas" oninput={() => (scrollTop = 0)} />
              <label class="check"><input type="checkbox" bind:checked={grouped} disabled={sortColumn !== null} />Agrupar por loja</label>
              {#if grid.sets > 1}
                <div class="seg" role="group" aria-label="Resultado">
                  {#each { length: grid.sets }, index (index)}
                    <button aria-pressed={gridSet === index} onclick={() => loadGrid(index)}>Resultado {index + 1}</button>
                  {/each}
                </div>
              {/if}
              <span class="grow"></span>
              <button class="btn" onclick={copy} disabled={!grid.rows.length}>Copiar</button>
              <button class="btn" onclick={() => exportAs("csv")} disabled={!grid.rows.length}>Exportar CSV</button>
              <button class="btn" onclick={() => exportAs("xlsx")} disabled={!grid.rows.length}>Exportar XLSX</button>
            </div>
            {#if notice}<div class="notice selectable" role="status">{notice}</div>{/if}
            {#if grid.rows.length}
              <div class="table-wrap grid" bind:clientHeight={viewport} onscroll={(event) => (scrollTop = event.currentTarget.scrollTop)}>
                <table>
                  <thead>
                    <tr>
                      {#each grid.columns as column, index (index)}
                        <th class:fixed={column.fixed} class:num={column.numeric} aria-sort={sortColumn === index ? (sortDescending ? "descending" : "ascending") : "none"}>
                          <button class="sort" onclick={() => sortBy(index)}>
                            {column.name}{#if sortColumn === index}{sortDescending ? " ↓" : " ↑"}{/if}
                          </button>
                        </th>
                      {/each}
                    </tr>
                  </thead>
                  <tbody>
                    {#if firstItem > 0}<tr class="spacer" style="height:{firstItem * ROW_HEIGHT}px"><td colspan={grid.columns.length}></td></tr>{/if}
                    {#each items.slice(firstItem, lastItem) as item, offset (firstItem + offset)}
                      {#if "group" in item}
                        <tr class="group"><td colspan={grid.columns.length}>{item.group} <span class="light">· {item.count}</span></td></tr>
                      {:else}
                        <tr>
                          {#each item.row as cell, index (index)}
                            <td class:mono={index === 0} class:data={!grid.columns[index].fixed || index === 2} class:muted={index === 2} class:num={grid.columns[index].numeric}>
                              {#if cell === null}<span class="null">NULL</span>{:else}{cell}{/if}
                            </td>
                          {/each}
                        </tr>
                      {/if}
                    {/each}
                    {#if lastItem < items.length}<tr class="spacer" style="height:{(items.length - lastItem) * ROW_HEIGHT}px"><td colspan={grid.columns.length}></td></tr>{/if}
                  </tbody>
                </table>
              </div>
            {:else}
              <div class="empty"><p>Nenhum equipamento devolveu linhas{grid.sets ? " neste resultado" : ""}.</p></div>
            {/if}
          {/if}
        {:else if tab === "falhas"}
          <div class="toolbar">
            <span class="muted small">Offline é comum fora do horário da loja. Falhas pedem atenção.</span>
            <span class="grow"></span>
            <button class="btn primary" onclick={retry} disabled={running || !(failedDevices.length + offlineDevices.length)}>Repetir só estes</button>
          </div>
          {#if failedDevices.length + offlineDevices.length}
            <div class="table-wrap grid">
              <table>
                <thead><tr><th>Loja</th><th>Equipamento</th><th>IP</th><th>Status</th><th>Detalhe</th></tr></thead>
                <tbody>
                  {#each [...failedDevices, ...offlineDevices] as [device, state] (device.id)}
                    <tr>
                      <td class="mono">{storeById.get(device.store_id)?.code ?? "?"}</td>
                      <td>{device.name}</td>
                      <td class="data muted">{device.host}</td>
                      <td><span class="pill {phaseClass(state.phase)}">{phaseLabel(state.phase)}</span></td>
                      <td class="detail selectable" title={state.failure?.detail ?? ""}>{state.failure?.message ?? ""}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {:else}
            <div class="empty"><p>Todos os equipamentos responderam.</p></div>
          {/if}
        {:else}
          <div class="table-wrap log selectable">
            {#each log as line, index (index)}<div><span class="muted">{clock(line.at)}</span> {line.text}</div>{/each}
          </div>
        {/if}
      {/if}
    </div>
  </div>
</section>

<style>
  .query { flex: 1; display: flex; min-height: 0; }
  .gone { display: none; }
  .small { font-size: var(--text-sm); }
  .pad { padding: 8px; margin: 0; }
  .grow { flex: 1; }

  .targets { width: 290px; flex: none; border-right: 1px solid var(--border); background: var(--surface); display: flex; flex-direction: column; min-height: 0; }
  .targets header { padding: 16px 16px 10px; }
  .targets h2 { font-size: var(--text-md); font-weight: 600; margin: 0 0 2px; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; padding: 0 16px 10px; }
  .search { padding: 0 16px 10px; }
  .store-list { flex: 1; overflow: auto; padding: 0 8px; }
  .store { display: grid; grid-template-columns: 20px 40px 1fr auto; align-items: center; gap: 6px; padding: 6px 8px; border-radius: var(--radius); cursor: pointer; font-size: var(--text-sm); }
  .store:hover { background: var(--surface-sunken); }
  .store input { margin: 0; accent-color: var(--brand); }
  .store .code { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); }
  .store .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .store .count { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .targets footer { border-top: 1px solid var(--border); padding: 10px 16px; display: flex; justify-content: space-between; }

  .work { flex: 1; min-width: 0; display: flex; flex-direction: column; min-height: 0; }
  .editor-area { padding: 16px 20px 12px; border-bottom: 1px solid var(--border); background: var(--bg); flex: none; }
  .editor-head { display: flex; align-items: center; gap: 12px; margin-bottom: 10px; }
  .editor { display: grid; grid-template-columns: 40px 1fr; border: 1px solid var(--border-strong); border-radius: var(--radius-lg); background: var(--surface); overflow: hidden; height: 150px; }
  .editor:focus-within { border-color: var(--brand); }
  .lines { background: var(--surface-sunken); color: var(--text-muted); font: 400 13px/20px var(--font-mono); text-align: right; padding: 10px 8px 10px 0; user-select: none; overflow: hidden; }
  .editor textarea { border: 0; outline: 0; resize: none; background: transparent; font: 400 13px/20px var(--font-mono); padding: 10px 12px; white-space: pre; overflow: auto; }
  .runbar { display: flex; align-items: center; gap: 12px; margin-top: 10px; font-size: var(--text-sm); flex-wrap: wrap; }
  .runbar strong { color: var(--text); font-weight: 500; }
  .btn.lg { padding: 8px 18px; font-size: var(--text-md); }

  .results { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .empty { margin: auto; text-align: center; color: var(--border-strong); padding: 40px; display: grid; justify-items: center; gap: 6px; }
  .empty h3 { color: var(--text); font-size: var(--text-lg); font-weight: 600; margin: 4px 0 0; }
  .empty p { color: var(--text-muted); margin: 0; max-width: 52ch; }
  .progress { padding: 12px 20px; display: flex; gap: 8px; overflow-x: auto; border-bottom: 1px solid var(--border); flex: none; }
  .tile { flex: none; width: 92px; border: 1px solid var(--border); border-radius: var(--radius); padding: 6px 8px; background: var(--surface); font-size: var(--text-xs); }
  .tile .t { display: flex; justify-content: space-between; font-weight: 600; font-variant-numeric: tabular-nums; }
  .tile .bar { display: flex; height: 5px; border-radius: 3px; overflow: hidden; background: var(--surface-sunken); margin-top: 6px; }
  .tile .bar span { display: block; height: 100%; transition: width 0.25s; }
  .summary { display: flex; align-items: center; gap: 14px; padding: 10px 20px; font-size: var(--text-sm); flex: none; }
  .toolbar { display: flex; align-items: center; gap: 10px; padding: 10px 20px; flex: none; }
  .toolbar .filter { width: 240px; }
  .notice { margin: 0 20px 8px; padding: 6px 10px; background: var(--surface-sunken); border-radius: var(--radius); font-size: var(--text-sm); font-family: var(--font-mono); word-break: break-all; }

  .grid { flex: 1; min-height: 0; margin: 0 20px 16px; }
  .grid th { padding: 0; }
  .grid th.fixed .sort { color: var(--brand); }
  .grid th.num .sort { text-align: right; }
  .sort { all: unset; display: block; width: 100%; box-sizing: border-box; padding: 7px 12px; cursor: pointer; }
  .sort:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
  .grid td { height: 30px; }
  .grid td.detail { white-space: normal; }
  .grid tr.group td { background: var(--bg); font-weight: 600; font-size: var(--text-xs); color: var(--text-muted); }
  .grid tr.spacer td { padding: 0; border: 0; height: auto; }
  .light { font-weight: 400; }
  .null { color: var(--text-muted); font-style: italic; }
  .log { flex: 1; min-height: 0; margin: 12px 20px 16px; padding: 12px 16px; font: 12.5px/1.8 var(--font-mono); }
</style>
