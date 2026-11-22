<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import {
    api,
    message,
    plural,
    type Device,
    type Engine,
    type ExportFormat,
    type Failure,
    type Fleet,
    type Grid,
    type ProgressEvent,
    type RunStatus,
    type Step,
    type CorrectionSummary,
    type Comparison,
    type SavedQuery,
    type SavedView,
    type Store,
    type Values,
    fileStamp,
    paramNames,
    when,
  } from "../lib/api";
  import EmptyState from "../lib/EmptyState.svelte";
  import Modal from "../lib/Modal.svelte";
  import SavedForm from "../lib/SavedForm.svelte";
  import SqlEditor from "../lib/SqlEditor.svelte";

  let {
    chainId,
    active,
    onaudit,
    onfleet,
    onbusy,
  }: {
    chainId: number;
    active: boolean;
    onaudit: () => void;
    onfleet: () => void;
    /** Avisa o App de execução em andamento ou correção entre etapas, que a troca de rede perderia. */
    onbusy?: (busy: { running: boolean; correcting: boolean }) => void;
  } = $props();

  type Phase = "queued" | "connecting" | "running" | "skipped" | RunStatus;
  type DeviceState = {
    phase: Phase;
    profile: string | null;
    failure: Failure | null;
    rows: number;
    truncated: boolean;
    warnings: number;
    affected: number;
  };
  type Item = { group: string; count: number } | { row: (string | null)[] };

  const ROW_HEIGHT = 30;
  const SQL_KEY = "selectall.sql";

  let fleet = $state<Fleet>({ stores: [], devices: [], profiles: [], rules: [] });
  let loadFailure = $state("");
  let fleetLoaded = $state(false);
  const chosenStores = new SvelteSet<number>();
  const hiddenKinds = new SvelteSet<string>();
  /** Equipamentos desmarcados dentro de lojas escolhidas; por padrão, todos vão. */
  const skippedDevices = new SvelteSet<number>();
  const openStores = new SvelteSet<number>();
  let search = $state("");
  let sql = $state(readSql());
  let editor = $state<SqlEditor>();

  let runId = $state(0);
  let starting = $state(false);
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
  let tab = $state<"grade" | "comparar" | "falhas" | "msgs">("grade");
  let comparison = $state<Comparison | null>(null);
  /** Colunas escolhidas para comparar; nulo segue o padrão do núcleo. */
  let compareColumns = $state<string[] | null>(null);
  let choosingColumns = $state(false);
  let compareFailure = $state("");

  /** Valores dos parâmetros `{{nome}}` do SQL, preenchidos antes de executar. */
  let values = $state<Values>({});
  /** Consulta salva aberta no editor; o nome vai ao histórico enquanto o SQL não muda. */
  let opened = $state<{ id: number; name: string; sql: string; profileId: number | null } | null>(null);
  let saveForm = $state<SavedQuery | null>(null);
  /** Lojas de execuções reabertas que não existem mais na frota. */
  let ghostStores = $state<Store[]>([]);
  let filter = $state("");
  let sortColumn = $state<number | null>(null);
  let sortDescending = $state(false);
  let grouped = $state(true);
  let notice = $state("");
  let scrollTop = $state(0);
  let viewport = $state(400);

  let mode = $state<"read" | "write">("read");
  /** Etapa da execução em andamento ou da última; nulo em leitura. */
  let runStep = $state<Step | null>(null);
  let summary = $state<CorrectionSummary | null>(null);
  let writeDevices = $state<Device[]>([]);
  let rehearsal = $state<Record<number, DeviceState>>({});
  let applied = $state<Record<number, DeviceState & { step: Step }>>({});
  let pilotAt = $state<Date | null>(null);
  let rolloutRan = $state(false);
  const excluded = new SvelteSet<number>();
  let pilotId = $state<number | null>(null);
  let skipRehearsal = $state(false);
  let skipWarning = $state(false);
  let confirming = $state(false);
  let confirmText = $state("");
  let writeTab = $state<"equipamentos" | "msgs">("equipamentos");

  let unlisten: UnlistenFn | undefined;

  const kinds = $derived([...new Set(fleet.devices.map((d) => d.kind))].sort((a, b) => a.localeCompare(b, "pt-BR")));
  const storeById = $derived(new Map([...fleet.stores, ...ghostStores].map((s) => [s.id, s])));
  const storeByCode = $derived(new Map(fleet.stores.map((s) => [s.code, s])));
  const profileById = $derived(new Map(fleet.profiles.map((p) => [p.id, p])));
  const visibleStores = $derived(
    fleet.stores.filter((s) => {
      const term = search.trim().toLowerCase();
      return !term || s.code.toLowerCase().includes(term) || s.name.toLowerCase().includes(term);
    }),
  );
  const targets = $derived(fleet.devices.filter((d) => chosenStores.has(d.store_id) && !hiddenKinds.has(d.kind) && !skippedDevices.has(d.id)));
  const chosenCount = $derived(new Set(targets.map((d) => d.store_id)).size);
  const profileSummary = $derived(summarizeProfiles(targets));
  /** Dialeto do destaque: o motor do perfil principal da maioria dos alvos. */
  const editorEngine = $derived.by((): Engine => {
    const counts = new Map<Engine, number>();
    for (const device of targets) {
      const engine = profileById.get(device.profile_ids[0])?.engine;
      if (engine) counts.set(engine, (counts.get(engine) ?? 0) + 1);
    }
    return [...counts].sort((a, b) => b[1] - a[1])[0]?.[0] ?? "mysql";
  });
  const params = $derived(paramNames(sql));
  const filled = $derived(Object.fromEntries(params.map((name) => [name, values[name] ?? ""])));
  const openedLabel = $derived(opened && opened.sql === sql ? opened.name : null);

  const finished = $derived(runDevices.map((d) => [d, states[d.id]] as const).filter(([, s]) => s && isFinal(s.phase)));
  const okDevices = $derived(finished.filter(([, s]) => s.phase === "success" || s.phase === "partial"));
  const offlineDevices = $derived(finished.filter(([, s]) => s.phase === "offline"));
  const stoppedDevices = $derived(finished.filter(([, s]) => s.phase === "skipped" || s.phase === "cancelled"));
  const failedDevices = $derived(finished.filter(([, s]) => s.phase === "failed" || s.phase === "timed_out" || (s.phase === "partial" && s.failure)));
  const pendingDevices = $derived([...failedDevices, ...stoppedDevices, ...offlineDevices]);
  const totalRows = $derived(okDevices.reduce((sum, [, s]) => sum + s.rows, 0));
  const tiles = $derived(storeTiles());

  const deviceById = $derived(new Map(writeDevices.map((d) => [d.id, d])));
  const rehearsedRows = $derived(new Map(summary?.rehearsed ?? []));
  const sqlChanged = $derived(
    summary !== null &&
      (!sql.includes(summary.sql) ||
        JSON.stringify(summary.values) !==
          JSON.stringify(sortedValues(Object.fromEntries(paramNames(summary.sql).map((name) => [name, values[name] ?? ""]))))),
  );
  const candidates = $derived(new Set(summary?.candidates ?? []));
  const remaining = $derived(
    (summary?.candidates ?? [])
      .filter((id) => !summary!.applied.includes(id) && !summary!.unknown.includes(id) && !excluded.has(id)),
  );
  const remainingRows = $derived(remaining.reduce((sum, id) => sum + (rehearsedRows.get(id) ?? 0), 0));
  const rehearsedTotal = $derived((summary?.rehearsed ?? []).reduce((sum, [, rows]) => sum + rows, 0));
  const stepIndex = $derived(
    !summary || sqlChanged || !summary.candidates.length ? 0 : !summary.pilot_applied ? 1 : rolloutRan ? 3 : 2,
  );
  const pilotOptions = $derived(writeDevices.filter((d) => candidates.has(d.id) && !excluded.has(d.id) && !summary?.applied.includes(d.id)));
  const pilotDone = $derived(Object.entries(applied).find(([, state]) => state.step === "pilot" && state.phase === "applied"));
  const notRehearsed = $derived(writeDevices.filter((d) => rehearsal[d.id] && isFinal(rehearsal[d.id].phase) && rehearsal[d.id].phase !== "rehearsed"));
  const rollout = $derived(Object.values(applied).filter((state) => state.step === "rollout"));

  const items = $derived(buildItems());
  const firstItem = $derived(Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - 10));
  const lastItem = $derived(Math.min(items.length, firstItem + Math.ceil(viewport / ROW_HEIGHT) + 20));

  $effect(() => {
    if (active) load(chainId);
  });

  $effect(() => {
    onbusy?.({ running: running || starting, correcting: stepIndex === 1 || stepIndex === 2 });
  });

  let destroyed = false;

  onMount(async () => {
    const stop = await listen<ProgressEvent>("query-progress", (event) => receive(event.payload));
    // Desmontado antes de o listen responder: solta o ouvinte na hora.
    if (destroyed) stop();
    else unlisten = stop;
  });

  onDestroy(() => {
    destroyed = true;
    unlisten?.();
  });

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
      // Nada vem marcado: em frota grande, um Ctrl+Enter distraído não deve alcançar todas as lojas.
      for (const storeId of [...chosenStores]) if (!fleet.stores.some((s) => s.id === storeId)) chosenStores.delete(storeId);
    } catch (error) {
      loadFailure = message(error);
    }
    fleetLoaded = true;
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

  /** Um equipamento vai à execução quando a loja está escolhida e ele não foi desmarcado. */
  function isPicked(device: Device): boolean {
    return chosenStores.has(device.store_id) && !skippedDevices.has(device.id);
  }

  function devicesOf(storeId: number): Device[] {
    return fleet.devices.filter((d) => d.store_id === storeId && !hiddenKinds.has(d.kind));
  }

  /** A caixa da loja reflete os equipamentos visíveis: todos, alguns ou nenhum. */
  function storeState(storeId: number): "all" | "some" | "none" {
    const all = devicesOf(storeId);
    const picked = all.filter(isPicked).length;
    if (!picked) return "none";
    return picked === all.length ? "all" : "some";
  }

  function setStore(storeId: number, on: boolean) {
    for (const device of fleet.devices) if (device.store_id === storeId) skippedDevices.delete(device.id);
    if (on) chosenStores.add(storeId);
    else chosenStores.delete(storeId);
  }

  /** Loja com todos marcados desmarca tudo; com alguns ou nenhum, marca todos. */
  function toggleStore(storeId: number) {
    setStore(storeId, storeState(storeId) !== "all");
  }

  /** Marcar um equipamento de loja desmarcada escolhe só ele; desmarcar o último solta a loja. */
  function toggleDevice(device: Device) {
    const storeId = device.store_id;
    if (!chosenStores.has(storeId)) {
      chosenStores.add(storeId);
      for (const other of fleet.devices) if (other.store_id === storeId && other.id !== device.id) skippedDevices.add(other.id);
      skippedDevices.delete(device.id);
    } else if (skippedDevices.has(device.id)) skippedDevices.delete(device.id);
    else skippedDevices.add(device.id);
    if (storeState(storeId) === "none") setStore(storeId, false);
  }

  function toggleOpen(id: number) {
    if (openStores.has(id)) openStores.delete(id);
    else openStores.add(id);
  }

  function countFor(storeId: number): string {
    const all = devicesOf(storeId);
    const picked = all.filter(isPicked).length;
    return picked && picked < all.length ? `${picked}/${all.length}` : String(all.length);
  }

  function isFinal(phase: Phase): boolean {
    return phase !== "queued" && phase !== "connecting" && phase !== "running";
  }

  function blank(): DeviceState {
    return { phase: "queued", profile: null, failure: null, rows: 0, truncated: false, warnings: 0, affected: 0 };
  }

  function note(text: string) {
    log.push({ at: new Date(), text });
  }

  function label(device: Device): string {
    const store = storeById.get(device.store_id);
    return `${store?.code ?? "?"} · ${device.name}`;
  }

  /** SQL a executar: a seleção do editor quando houver, senão o texto inteiro. */
  function scriptToRun(): { text: string; values: Values; partial: boolean } {
    const selected = editor?.selection() ?? "";
    const text = selected.trim() ? selected : sql;
    const used: Values = Object.fromEntries(paramNames(text).map((name) => [name, values[name] ?? ""]));
    return { text, values: used, partial: text !== sql };
  }

  async function run() {
    if (running || starting || !targets.length || !sql.trim()) return;
    const script = scriptToRun();
    const missing = Object.keys(script.values).filter((name) => !script.values[name]);
    if (missing.length) {
      runFailure = `Preencha ${missing.length === 1 ? "o parâmetro" : "os parâmetros"} ${missing.join(", ")}.`;
      return;
    }
    rememberSql();
    if (mode === "write") return skipRehearsal ? prepareWithoutRehearsal(script) : rehearse(script);
    const devices = [...targets];
    const ids = devices.map((d) => d.id);
    const perStore = await api.perStoreLimit().catch(() => null);
    await begin(() => api.runQuery(chainId, ids, script.text, script.values, script.partial ? null : openedLabel), () => {
      runStep = null;
      runDevices = devices;
      states = Object.fromEntries(ids.map((id) => [id, blank()]));
      log = [];
      grid = null;
      gridSet = 0;
      sortColumn = null;
      comparison = null;
      compareColumns = null;
      ghostStores = [];
      const limit = perStore ? `, até 16 por vez e ${perStore} por loja` : ", até 16 por vez";
      note(`Execução iniciada em ${plural(devices.length, "equipamento", "equipamentos")} de ${plural(new Set(devices.map((d) => d.store_id)).size, "loja", "lojas")}${limit}.`);
      if (script.partial) note("Executando só o trecho selecionado no editor.");
      note("Modo somente leitura: transação READ ONLY exigida pelo servidor.");
    });
  }

  function startWrite(step: Step, devices: Device[]) {
    runStep = step;
    runDevices = devices;
    states = Object.fromEntries(devices.map((d) => [d.id, blank()]));
    const count = plural(devices.length, "equipamento", "equipamentos");
    if (step === "rehearsal") note(`Ensaio iniciado em ${count}: cada um em transação, desfeita no fim.`);
    if (step === "pilot") note(`Piloto iniciado em ${devices.map(label).join(", ")}: a transação será confirmada.`);
    if (step === "rollout") note(`Lote iniciado em ${count}, um COMMIT por equipamento.`);
  }

  async function rehearse(script: { text: string; values: Values; partial: boolean }) {
    const devices = [...targets];
    await begin(() => api.rehearse(chainId, devices.map((d) => d.id), script.text, script.values), () => {
      resetCorrection(devices);
      startWrite("rehearsal", devices);
      if (script.partial) note("Ensaiando só o trecho selecionado no editor.");
    });
  }

  function resetCorrection(devices: Device[]) {
    log = [];
    writeDevices = devices;
    rehearsal = {};
    applied = {};
    summary = null;
    excluded.clear();
    pilotId = null;
    pilotAt = null;
    rolloutRan = false;
    writeTab = "equipamentos";
  }

  /** Sem ensaio: nada roda agora; o próximo passo é o piloto num dos escolhidos. */
  async function prepareWithoutRehearsal(script: { text: string; values: Values }) {
    const devices = [...targets];
    runFailure = "";
    try {
      await api.skipRehearsal(chainId, devices.map((d) => d.id), script.text, script.values);
      resetCorrection(devices);
      runDevices = devices;
      states = {};
      summary = await api.correction();
      note(`Ensaio pulado em ${plural(devices.length, "equipamento", "equipamentos")}: sem prévia de linhas. O piloto grava de verdade.`);
    } catch (error) {
      runFailure = message(error);
    }
  }

  function toggleSkip(event: Event & { currentTarget: HTMLInputElement }) {
    if (event.currentTarget.checked) {
      event.currentTarget.checked = false;
      skipWarning = true;
    } else skipRehearsal = false;
  }

  async function applyPilot() {
    const device = writeDevices.find((d) => d.id === (pilotId ?? pilotOptions[0]?.id));
    if (running || !device) return;
    await begin(() => api.applyPilot(chainId, [device.id]), () => startWrite("pilot", [device]));
  }

  function openConfirm() {
    confirmText = "";
    confirming = true;
  }

  async function applyRollout() {
    const devices = remaining.map((id) => deviceById.get(id)).filter((d): d is Device => d !== undefined);
    const typed = Number(confirmText.trim());
    confirming = false;
    await begin(() => api.applyRollout(chainId, [...excluded], typed), () => startWrite("rollout", devices));
  }

  async function abandon() {
    await api.abandonCorrection();
    note("Correção encerrada depois do piloto; o lote não foi aplicado.");
    summary = null;
    rolloutRan = false;
  }

  function toggleExcluded(id: number) {
    if (excluded.has(id)) excluded.delete(id);
    else excluded.add(id);
  }

  function setMode(next: "read" | "write") {
    if (running) return;
    mode = next;
  }

  async function retry() {
    const existing = new Set(fleet.devices.map((d) => d.id));
    const devices = pendingDevices.map(([d]) => d).filter((d) => existing.has(d.id));
    const gone = pendingDevices.length - devices.length;
    if (running || starting) return;
    if (!devices.length) {
      runFailure = "Os equipamentos a repetir não existem mais na frota.";
      return;
    }
    const ids = devices.map((d) => d.id);
    await begin(() => api.retryQuery(chainId, ids), () => {
      runStep = null;
      for (const id of ids) states[id] = blank();
      note(`Repetindo em ${plural(devices.length, "equipamento", "equipamentos")}; os demais resultados foram mantidos.`);
      if (gone) note(`${plural(gone, "equipamento ficou", "equipamentos ficaram")} de fora por não existir mais na frota.`);
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
          affected: event.affected_rows,
        };
        const device = runDevices.find((d) => d.id === event.device_id);
        if (device) describeOutcome(device, states[event.device_id]);
        if (runStep === "rehearsal") rehearsal[event.device_id] = states[event.device_id];
        else if (runStep) applied[event.device_id] = { ...states[event.device_id], step: runStep };
        break;
      }
      case "skipped":
        states[event.device_id] = { ...blank(), phase: "skipped", failure: { message: "Não iniciado: a execução foi cancelada.", detail: null } };
        if (runStep === "rehearsal") rehearsal[event.device_id] = states[event.device_id];
        else if (runStep) applied[event.device_id] = { ...states[event.device_id], step: runStep };
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
    if (state.profile && first && state.profile !== first) note(`${label(device)} não aceitou ${first}; respondeu com ${state.profile}.`);
    if (state.truncated) note(`${label(device)}: resultado cortado no limite de 1.000 linhas.`);
    if (state.warnings) note(`${label(device)}: ${plural(state.warnings, "aviso", "avisos")} do servidor.`);
    if (state.phase === "rehearsed") note(`${label(device)}: o ensaio alteraria ${plural(state.affected, "linha", "linhas")}; desfeito.`);
    if (state.phase === "applied") note(`${label(device)}: ${plural(state.affected, "linha alterada", "linhas alteradas")}, COMMIT confirmado.`);
    if (runStep && state.failure) note(`${label(device)}: ${state.failure.message}`);
  }

  async function finish() {
    running = false;
    cancelling = false;
    elapsed = (performance.now() - startedAt) / 1000;
    finishedAt = new Date();
    if (offlineDevices.length) note(`${plural(offlineDevices.length, "equipamento não respondeu", "equipamentos não responderam")} na rede; marcados como offline.`);
    if (stoppedDevices.length) note(`${plural(stoppedDevices.length, "equipamento ficou", "equipamentos ficaram")} sem resultado completo por causa do cancelamento.`);
    if (failedDevices.length) note(`${plural(failedDevices.length, "equipamento falhou", "equipamentos falharam")}; veja Offline e falhas.`);
    note(`Concluído em ${seconds(elapsed)}.`);
    if (!runStep) return loadGrid(gridSet);
    if (runStep === "pilot") pilotAt = finishedAt;
    if (runStep === "rollout") rolloutRan = true;
    try {
      summary = await api.correction();
    } catch (error) {
      runFailure = message(error);
    }
  }

  async function loadGrid(set: number) {
    try {
      grid = await api.grid(set);
      gridSet = set;
      sortColumn = null;
      scrollTop = 0;
      comparison = null;
      if (tab === "comparar") await loadComparison();
    } catch (error) {
      runFailure = message(error);
    }
  }

  async function loadComparison() {
    try {
      comparison = await api.compare(gridSet, compareColumns);
      compareFailure = "";
    } catch (error) {
      compareFailure = message(error);
    }
  }

  function showComparison() {
    tab = "comparar";
    if (!comparison && grid) loadComparison();
  }

  function toggleCompareColumn(name: string) {
    if (!comparison) return;
    const chosen = comparison.columns.filter((c) => c.chosen).map((c) => c.name);
    compareColumns = chosen.includes(name) ? chosen.filter((c) => c !== name) : [...chosen, name];
    loadComparison();
  }

  function groupTitle(index: number): string {
    if (comparison?.groups.length === 1) return "Todos iguais";
    if (!comparison?.majority) return `Sem maioria · grupo ${index + 1}`;
    return index === 0 ? "Maioria" : "Diverge da maioria";
  }

  function groupValues(rows: (string | null)[][], total: number): string[] {
    const names = comparison?.columns.filter((c) => c.chosen).map((c) => c.name) ?? [];
    if (!total) return ["Sem linhas"];
    const lines = rows.map((row) => row.map((value, index) => `${names[index]} = ${value ?? "NULL"}`).join(" · "));
    if (total > rows.length) lines.push(`e mais ${plural(total - rows.length, "linha", "linhas")}`);
    return lines;
  }

  function storeShare(share: { store: string; count: number; total: number }): string {
    const name = storeByCode.get(share.store)?.name;
    const label = name ? `${share.store} ${name}` : share.store;
    return `${label}: ${share.count === share.total ? (share.total === 1 ? "o único" : `todos os ${share.total}`) : `${share.count} de ${share.total}`}`;
  }

  function sortedValues(input: Values): Values {
    return Object.fromEntries(Object.entries(input).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
  }

  /** Coloca uma consulta salva no editor, no modo certo, com os parâmetros a preencher. */
  export function openSaved(query: SavedView) {
    if (running) {
      runFailure = "Espere a execução em andamento terminar para abrir outra consulta.";
      return;
    }
    sql = query.sql;
    rememberSql();
    mode = query.write ? "write" : "read";
    if (!query.write) skipRehearsal = false;
    opened = { id: query.id, name: query.name, sql: query.sql, profileId: query.profile_id };
    values = Object.fromEntries(query.params.map((name) => [name, values[name] ?? ""]));
    runFailure = "";
  }

  /** Mostra uma execução do histórico na grade, como se tivesse acabado de rodar. */
  export async function reopen(id: number) {
    if (running) {
      runFailure = "Espere a execução em andamento terminar para reabrir outra.";
      return;
    }
    try {
      const { entry, devices } = await api.reopenHistory(id);
      mode = "read";
      opened = null;
      if (entry.sql !== null) {
        sql = entry.sql;
        values = { ...entry.values };
      }
      const ghosts = new Map<string, Store>();
      runDevices = devices.map((d, index) => {
        const known = fleet.devices.find((x) => x.id === d.device_id && x.host === d.host);
        if (known) return known;
        let store = storeByCode.get(d.store) ?? ghosts.get(d.store);
        if (!store) {
          store = { id: -(ghosts.size + 1), chain_id: chainId, code: d.store, name: "", subnet: "" };
          ghosts.set(d.store, store);
        }
        return { id: d.device_id || -(index + 1), store_id: store.id, name: d.device, kind: "", host: d.host, profile_ids: [] };
      });
      ghostStores = [...ghosts.values()];
      states = Object.fromEntries(
        devices.map((d) => [
          d.device_id,
          { phase: d.status, profile: d.profile, failure: d.failure, rows: d.rows, truncated: d.truncated, warnings: d.warnings, affected: d.affected_rows },
        ]),
      );
      runStep = null;
      runFailure = "";
      notice = "";
      elapsed = entry.elapsed_ms / 1000;
      finishedAt = new Date(entry.at);
      log = [{ at: new Date(), text: `Execução de ${when(entry.at)} reaberta do histórico${entry.sql === null ? "; o SQL não foi guardado porque cita senha, token ou segredo" : ""}.` }];
      if (entry.targets > devices.length) log.push({ at: new Date(), text: `${plural(entry.targets - devices.length, "equipamento ficou", "equipamentos ficaram")} sem resultado por causa do cancelamento.` });
      comparison = null;
      compareColumns = null;
      tab = "grade";
      await loadGrid(0);
    } catch (error) {
      runFailure = message(error);
    }
  }

  function openSaveForm() {
    const current = opened && opened.sql === sql ? opened : null;
    saveForm = {
      id: current?.id ?? 0,
      chain_id: chainId,
      name: current?.name ?? "",
      description: "",
      profile_id: current?.profileId ?? null,
      sql,
    };
  }

  function saved(view: SavedView) {
    saveForm = null;
    sql = view.sql;
    rememberSql();
    opened = { id: view.id, name: view.name, sql: view.sql, profileId: view.profile_id };
    notice = `Consulta "${view.name}" salva na biblioteca.`;
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
        ok: (count("success", "partial", "rehearsed", "applied") / total) * 100,
        err: (count("failed", "timed_out", "unknown") / total) * 100,
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
    const stem = `selectall-${fileStamp()}`;
    try {
      const path = await api.exportGrid(gridSet, format, stem);
      notice = `Salvo em ${path}`;
    } catch (error) {
      notice = message(error);
    }
  }

  function phaseLabel(phase: Phase): string {
    return {
      queued: "Na fila",
      connecting: "Conectando",
      running: "Executando",
      skipped: "Não iniciado",
      success: "Respondeu",
      rehearsed: "Ensaio desfeito",
      applied: "Aplicado",
      unknown: "Resultado desconhecido",
      partial: "Parcial",
      failed: "Falhou",
      offline: "Offline",
      cancelled: "Cancelado",
      timed_out: "Prazo esgotado",
    }[phase];
  }

  function phaseClass(phase: Phase): string {
    if (phase === "failed" || phase === "timed_out") return "err";
    if (phase === "partial" || phase === "unknown") return "warn";
    if (phase === "success" || phase === "applied") return "ok";
    if (phase === "rehearsed") return "run";
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
        {@const storeDevices = devicesOf(store.id)}
        {@const state = storeState(store.id)}
        {@const open = openStores.has(store.id)}
        <div class="store" class:open>
          <label class="store-pick">
            <input type="checkbox" checked={state === "all"} indeterminate={state === "some"} disabled={!storeDevices.length} onchange={() => toggleStore(store.id)} />
            <span class="code">{store.code}</span>
            <span class="name">{store.name}</span>
          </label>
          <button
            class="expand"
            aria-expanded={open}
            aria-controls="devices-{store.id}"
            aria-label="{open ? 'Ocultar' : 'Mostrar'} equipamentos da loja {store.code}"
            title={open ? "Ocultar equipamentos" : "Escolher equipamentos"}
            disabled={!storeDevices.length}
            onclick={() => toggleOpen(store.id)}>
            <span class="count" class:partial={state === "some"}>{countFor(store.id)}</span>
            <span class="chevron" aria-hidden="true">{open ? "▾" : "▸"}</span>
          </button>
        </div>
        {#if open}
          <div class="device-list" id="devices-{store.id}" role="group" aria-label="Equipamentos da loja {store.code}">
            {#each storeDevices as device (device.id)}
              <label class="device" class:off={!isPicked(device)}>
                <input type="checkbox" checked={isPicked(device)} onchange={() => toggleDevice(device)} />
                <span class="name">{device.name}</span>
                <span class="muted mono">{device.host}</span>
              </label>
            {/each}
          </div>
        {/if}
      {:else}
        {#if fleet.stores.length}
          <p class="muted small pad">Nenhuma loja com esse termo.</p>
        {:else if fleetLoaded}
          <p class="muted small pad">Nenhuma loja cadastrada. <button class="link" onclick={onfleet}>Ir para a Frota</button></p>
        {/if}
      {/each}
    </div>
    <footer>
      <button class="btn ghost" onclick={() => visibleStores.forEach((s) => setStore(s.id, true))}>Todas</button>
      <button class="btn ghost" onclick={() => visibleStores.forEach((s) => setStore(s.id, false))}>Nenhuma</button>
    </footer>
  </aside>

  <div class="work">
    <div class="editor-area">
      <div class="editor-head">
        <div class="seg" role="group" aria-label="Modo">
          <button aria-pressed={mode === "read"} disabled={running} onclick={() => setMode("read")}>Consultar</button>
          <button aria-pressed={mode === "write"} disabled={running} onclick={() => setMode("write")}>Corrigir</button>
        </div>
        <span class="muted small">
          {mode === "read" ? "Somente leitura: o servidor recusa qualquer alteração." : "Correção em três etapas: ensaio, piloto e lote. Tudo vai para a auditoria."}
        </span>
        <span class="grow"></span>
        {#if openedLabel}<span class="muted small opened" title="Consulta salva aberta no editor">{openedLabel}</span>{/if}
        <button class="btn small" onclick={openSaveForm} disabled={!sql.trim()}>{openedLabel ? "Salvar alterações" : "Salvar consulta"}</button>
      </div>
      <div class="editor">
        <SqlEditor bind:this={editor} bind:value={sql} engine={editorEngine} onrun={run} />
      </div>
      {#if params.length}
        <div class="params" role="group" aria-label="Parâmetros">
          {#each params as name (name)}
            <label class="param">
              <span class="mono">{"{{"}{name}{"}}"}</span>
              <input class="input" value={values[name] ?? ""} oninput={(e) => (values[name] = e.currentTarget.value)} aria-invalid={!filled[name]} placeholder="valor" />
            </label>
          {/each}
          <span class="muted small">Cada valor entra como texto citado no dialeto de cada motor.</span>
        </div>
      {/if}
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
        {#if mode === "write" && !running}
          <label class="check skip" title="Aplica direto no piloto, sem a transação desfeita do ensaio">
            <input type="checkbox" checked={skipRehearsal} onchange={toggleSkip} />Pular ensaio
          </label>
        {/if}
        {#if !targets.length && !running}<span class="muted">Escolha as lojas à esquerda.</span>{/if}
        <span class="muted" aria-hidden="true"><span class="kbd">Ctrl</span> + <span class="kbd">Enter</span></span>
        {#if running}
          <button class="btn lg" onclick={cancel} disabled={cancelling}>{cancelling ? "Cancelando" : "Cancelar"}</button>
        {:else}
          <button class="btn primary lg" onclick={run} disabled={starting || !targets.length || !sql.trim()} aria-keyshortcuts="Control+Enter">
            {mode === "read" ? "Executar" : skipRehearsal ? (stepIndex === 0 ? "Preparar sem ensaio" : "Recomeçar sem ensaio") : stepIndex === 0 ? "Fazer ensaio" : "Refazer ensaio"}{#if targets.length}{" "}em {plural(targets.length, "equipamento", "equipamentos")}{/if}
          </button>
        {/if}
      </div>
    </div>

    <div class="results">
      {#if mode === "write"}
        {@render correction()}
      {:else if !runDevices.length}
        {#if fleetLoaded && !fleet.devices.length}
          <EmptyState title="Cadastre a frota para consultar">
            <ol>
              <li>Em Frota, crie um perfil de sistema: motor, porta, banco e usuário de leitura.</li>
              <li>Adicione as lojas pela regra de endereçamento, colando uma lista ou por planilha.</li>
              <li>Volte aqui, escolha as lojas e execute.</li>
            </ol>
            {#snippet actions()}<button class="btn primary" onclick={onfleet}>Ir para a Frota</button>{/snippet}
          </EmptyState>
        {:else}
          <EmptyState title="Escolha lojas e execute">
            <p>O resultado de todos os equipamentos aparece numa grade só, com loja e equipamento em cada linha.</p>
            <p><span class="kbd">Ctrl</span> + <span class="kbd">Enter</span> executa. Com um trecho selecionado no editor, executa só o trecho.</p>
          </EmptyState>
        {/if}
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
            {#if stoppedDevices.length}<span class="pill off">{stoppedDevices.length} {stoppedDevices.length === 1 ? "cancelado" : "cancelados"}</span>{/if}
            {#if failedDevices.length}<span class="pill err">{failedDevices.length} {failedDevices.length === 1 ? "falhou" : "falharam"}</span>{/if}
            <span class="muted">{plural(totalRows, "linha", "linhas")} · {seconds(elapsed)}{#if finishedAt}{` · executado às ${clock(finishedAt)}`}{/if}</span>
          {/if}
        </div>
        <div class="tabs" role="tablist">
          <button class="tab" role="tab" aria-selected={tab === "grade"} onclick={() => (tab = "grade")}>Resultado<span class="n">{(grid?.rows.length ?? 0).toLocaleString("pt-BR")}</span></button>
          <button class="tab" role="tab" aria-selected={tab === "comparar"} onclick={showComparison}>Comparar{#if comparison}<span class="n">{plural(comparison.groups.length, "grupo", "grupos")}</span>{/if}</button>
          <button class="tab" role="tab" aria-selected={tab === "falhas"} onclick={() => (tab = "falhas")}>Offline e falhas<span class="n">{pendingDevices.length}</span></button>
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
        {:else if tab === "comparar"}
          {@render compare()}
        {:else if tab === "falhas"}
          <div class="toolbar">
            <span class="muted small">Offline é comum fora do horário da loja. Falhas pedem atenção.</span>
            <span class="grow"></span>
            <button class="btn primary" onclick={retry} disabled={running || !pendingDevices.length}>Repetir só estes</button>
          </div>
          {#if pendingDevices.length}
            <div class="table-wrap grid plain">
              <table>
                <thead><tr><th>Loja</th><th>Equipamento</th><th>IP</th><th>Status</th><th>Detalhe</th></tr></thead>
                <tbody>
                  {#each pendingDevices as [device, state] (device.id)}
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

{#snippet compare()}
  {#if !grid}
    <div class="empty"><p>{running ? "A comparação aparece quando todos os equipamentos terminarem." : "Sem resultado."}</p></div>
  {:else if compareFailure}
    <p class="error-text pad" role="alert">{compareFailure}</p>
  {:else if comparison}
    {@const chosen = comparison.columns.filter((c) => c.chosen)}
    {@const ignored = comparison.columns.filter((c) => !c.chosen)}
    <div class="toolbar">
      <span class="muted small">
        {#if chosen.length}Comparando {#each chosen as column, index (column.name)}{index ? ", " : ""}<span class="mono">{column.name}</span>{/each}.{:else}Nenhuma coluna escolhida: todos os equipamentos ficam no mesmo grupo.{/if}
        {#if ignored.length}{" "}Fora: {ignored.map((c) => c.name).join(", ")}{ignored.some((c) => c.temporal) && compareColumns === null ? " (data e hora)" : ""}.{/if}
        {plural(comparison.compared, "equipamento comparado", "equipamentos comparados")}; offline e falhas ficam de fora.
      </span>
      <span class="grow"></span>
      <button class="btn" aria-expanded={choosingColumns} onclick={() => (choosingColumns = !choosingColumns)}>Escolher colunas</button>
    </div>
    {#if choosingColumns}
      <div class="chips columns" role="group" aria-label="Colunas comparadas">
        {#each comparison.columns as column (column.name)}
          <button class="chip" aria-pressed={column.chosen} onclick={() => toggleCompareColumn(column.name)}>{column.name}{#if column.temporal}<span class="muted"> · data</span>{/if}</button>
        {/each}
        {#if compareColumns !== null}<button class="btn ghost small" onclick={() => ((compareColumns = null), loadComparison())}>Voltar ao padrão</button>{/if}
      </div>
    {/if}
    {#if comparison.groups.length}
      <div class="compare">
        {#each comparison.groups as group, index (index)}
          {@const minor = !comparison.majority || index > 0}
          <div class="card group-card" class:minor>
            <div>
              <div class="section-title">{groupTitle(index)}</div>
              <div class="vals selectable">{#each groupValues(group.rows, group.row_count) as line, l (l)}<div>{line}</div>{/each}</div>
            </div>
            <div class="share">{group.members.length.toLocaleString("pt-BR")}<span class="muted">&nbsp;equip.</span></div>
            <div class="who">
              {#if minor}
                {group.stores.map(storeShare).join(" · ")}
              {:else}
                {plural(group.stores.length, "loja", "lojas")}
              {/if}
              <details>
                <summary>Ver equipamentos</summary>
                <div class="members selectable">{#each group.members as member, m (m)}<span>{member.store} · {member.device} <span class="mono muted">{member.host}</span></span>{/each}</div>
              </details>
            </div>
          </div>
        {/each}
      </div>
    {:else}
      <div class="empty"><p>Nenhum equipamento respondeu para comparar.</p></div>
    {/if}
  {/if}
{/snippet}

{#if saveForm}
  <SavedForm query={saveForm} profiles={fleet.profiles} onsaved={saved} onclose={() => (saveForm = null)} />
{/if}

{#snippet correction()}
  {#if !writeDevices.length && skipRehearsal}
    <div class="callout bad">
      <div class="grow"><strong>Ensaio desligado.</strong> O SQL vai direto ao piloto e grava de verdade, sem ver antes quantas linhas cada equipamento alteraria. Desmarque "Pular ensaio" para voltar ao fluxo seguro.</div>
    </div>
  {:else if !writeDevices.length}
    <EmptyState title="Correções começam por um ensaio">
      <p>O ensaio roda em transação e desfaz tudo. Você vê quantas linhas cada equipamento alteraria antes de gravar qualquer coisa.</p>
      <p>Depois vem o piloto, num equipamento só, e por fim o lote no restante. Cada etapa vai para a Auditoria.</p>
    </EmptyState>
  {:else}
    <div class="stepper" aria-label="Etapas da correção">
      {#each [["Ensaio", "transação desfeita"], ["Piloto", "1 equipamento"], ["Lote", "o restante"]] as [name, hint], index (name)}
        <div class="step" class:done={index < stepIndex} class:current={index === stepIndex}>
          <span class="b">{index === 0 && summary?.rehearsal_skipped ? "–" : index < stepIndex ? "✓" : index + 1}</span>{name}<span class="muted hint">· {index === 0 && summary?.rehearsal_skipped ? "pulado" : hint}</span>
        </div>
      {/each}
    </div>
    {#if running}
      <div class="callout info"><div class="grow"><strong>{runStep === "rehearsal" ? "Ensaio" : runStep === "pilot" ? "Piloto" : "Lote"} em andamento:</strong> {finished.length} de {runDevices.length} equipamentos.</div></div>
    {:else if sqlChanged}
      <div class="callout attn"><div class="grow">{#if summary?.rehearsal_skipped}<strong>O SQL mudou depois de preparar a correção.</strong> Prepare de novo antes de aplicar.{:else}<strong>O SQL mudou depois do ensaio.</strong> Faça um novo ensaio antes de aplicar.{/if}</div></div>
    {:else if summary}
      {#if summary.rehearsal_skipped}
        <div class="callout bad">
          <div class="grow"><strong>Correção sem ensaio.</strong> Não há prévia de linhas nem aviso de equipamento fora do padrão. Piloto e lote gravam de verdade; confira o piloto no equipamento antes do lote.</div>
        </div>
      {/if}
      {#if stepIndex === 1 && summary.rehearsal_skipped}
        {#if pilotOptions.length}
          <div class="callout info">
            <div class="grow">Escolha o equipamento piloto: a correção será aplicada e confirmada nele.</div>
            <select class="input pilot" aria-label="Piloto" value={pilotId ?? pilotOptions[0].id} onchange={(event) => (pilotId = Number(event.currentTarget.value))}>
              {#each pilotOptions as device (device.id)}<option value={device.id}>{label(device)}</option>{/each}
            </select>
            <button class="btn danger" onclick={applyPilot}>Aplicar no piloto sem ensaio</button>
          </div>
        {/if}
      {:else if stepIndex === 1}
        <div class="callout attn">
          <div class="grow">
            <strong>O ensaio alteraria {plural(rehearsedTotal, "linha", "linhas")} em {plural(summary.rehearsed.length, "equipamento", "equipamentos")}.</strong> Nada foi gravado.
            {#if notRehearsed.length}{plural(notRehearsed.length, "equipamento ficou", "equipamentos ficaram")} fora: sem ensaio, não recebe a correção.{/if}
          </div>
        </div>
        {#each summary.outliers as id (id)}
          <div class="callout attn">
            <div class="grow">{deviceById.get(id) ? label(deviceById.get(id)!) : id} alteraria {plural(rehearsedRows.get(id) ?? 0, "linha", "linhas")}; a mediana é {summary.median.toLocaleString("pt-BR")}.</div>
            <button class="btn" onclick={() => toggleExcluded(id)}>{excluded.has(id) ? "Devolver ao lote" : "Tirar do lote"}</button>
          </div>
        {/each}
        {#if pilotOptions.length}
          <div class="callout info">
            <div class="grow">Próximo passo: aplicar de verdade em um equipamento piloto e conferir.</div>
            <select class="input pilot" aria-label="Piloto" value={pilotId ?? pilotOptions[0].id} onchange={(event) => (pilotId = Number(event.currentTarget.value))}>
              {#each pilotOptions as device (device.id)}<option value={device.id}>{label(device)} · {plural(rehearsedRows.get(device.id) ?? 0, "linha", "linhas")}</option>{/each}
            </select>
            <button class="btn primary" onclick={applyPilot}>Aplicar no piloto</button>
          </div>
        {/if}
      {:else if stepIndex >= 2}
        {#if pilotDone}
          {@const device = deviceById.get(Number(pilotDone[0]))}
          <div class="callout good">
            <div class="grow"><strong>Piloto aplicado em {device ? label(device) : pilotDone[0]}:</strong> {plural(pilotDone[1].affected, "linha alterada", "linhas alteradas")}, transação confirmada{pilotAt ? ` às ${clock(pilotAt)}` : ""}. Confira no equipamento antes de seguir.</div>
          </div>
        {/if}
        {#if stepIndex === 3}
          {@const ok = rollout.filter((r) => r.phase === "applied")}
          <div class="callout {rollout.length === ok.length ? 'good' : 'attn'}">
            <div class="grow">
              <strong>Lote concluído:</strong> {plural(ok.length, "equipamento confirmado", "equipamentos confirmados")}, {plural(ok.reduce((sum, r) => sum + r.affected, 0), "linha", "linhas")}.
              {#if rollout.length > ok.length}{plural(rollout.length - ok.length, "equipamento não recebeu", "equipamentos não receberam")} a correção; veja a situação na tabela.{/if}
            </div>
            {#if remaining.length}<button class="btn" onclick={openConfirm}>Repetir o que faltou</button>{/if}
            <button class="btn" onclick={onaudit}>Ver auditoria</button>
          </div>
        {:else}
          <div class="callout info">
            <div class="grow">Lote: {plural(remaining.length, "equipamento restante", "equipamentos restantes")}, {summary.rehearsal_skipped ? "sem previsão de linhas (ensaio pulado)" : `${plural(remainingRows, "linha prevista", "linhas previstas")} no ensaio`}.</div>
            <button class="btn" onclick={abandon}>Parar aqui</button>
            <button class="btn primary" onclick={openConfirm} disabled={!remaining.length}>Aplicar no lote</button>
          </div>
        {/if}
      {/if}
      {#if summary.unknown.length}
        <div class="callout bad">
          <div class="grow"><strong>Resultado desconhecido em {plural(summary.unknown.length, "equipamento", "equipamentos")}:</strong> a conexão caiu depois do COMMIT. Confira no equipamento se a alteração foi gravada; ele não volta ao lote.</div>
        </div>
      {/if}
    {/if}
    <div class="tabs" role="tablist">
      <button class="tab" role="tab" aria-selected={writeTab === "equipamentos"} onclick={() => (writeTab = "equipamentos")}>Equipamentos<span class="n">{writeDevices.length}</span></button>
      <button class="tab" role="tab" aria-selected={writeTab === "msgs"} onclick={() => (writeTab = "msgs")}>Mensagens<span class="n">{log.length}</span></button>
    </div>
    {#if writeTab === "equipamentos"}
      <div class="table-wrap grid plain devices">
        <table>
          <thead><tr><th>Loja</th><th>Equipamento</th><th>IP</th><th class="num">Linhas no ensaio</th><th>Situação</th><th>Detalhe</th></tr></thead>
          <tbody>
            {#each writeDevices as device (device.id)}
              {@const state = applied[device.id] ?? rehearsal[device.id] ?? (runStep === "rehearsal" ? states[device.id] : undefined)}
              {@const rows = rehearsal[device.id]}
              <tr class:excluded={excluded.has(device.id)}>
                <td class="mono">{storeById.get(device.store_id)?.code ?? "?"}</td>
                <td>{device.name}</td>
                <td class="data muted">{device.host}</td>
                <td class="num">{rows?.phase === "rehearsed" ? rows.affected.toLocaleString("pt-BR") : "—"}{#if summary?.outliers.includes(device.id)} <span class="pill warn">fora do padrão</span>{/if}</td>
                <td>
                  {#if state}
                    <span class="pill {phaseClass(state.phase)}">{phaseLabel(state.phase)}{applied[device.id]?.step === "pilot" && state.phase === "applied" ? " (piloto)" : ""}</span>
                  {/if}
                  {#if excluded.has(device.id)}<span class="pill off">Fora do lote</span>{/if}
                </td>
                <td class="detail selectable" title={state?.failure?.detail ?? ""}>{state?.failure?.message ?? ""}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else}
      <div class="table-wrap log selectable">
        {#each log as line, index (index)}<div><span class="muted">{clock(line.at)}</span> {line.text}</div>{/each}
      </div>
    {/if}
  {/if}
{/snippet}

{#if confirming}
  <Modal title="Aplicar em {plural(remaining.length, 'equipamento', 'equipamentos')}?" width={560} onclose={() => (confirming = false)}>
    <p class="confirm-text">
      {summary?.rehearsal_skipped ? "Sem ensaio: não há previsão de quantas linhas serão alteradas." : `O ensaio indicou ${plural(remainingRows, "linha", "linhas")}.`} Cada equipamento terá sua própria transação; falhas são desfeitas e podem ser repetidas depois.
    </p>
    <pre class="confirm-sql selectable">{summary?.sql}{#each Object.entries(summary?.values ?? {}) as [name, value] (name)}{`\n-- {{${name}}} = ${value}`}{/each}</pre>
    <form
      id="confirm-form"
      onsubmit={(event) => {
        event.preventDefault();
        if (confirmText.trim() === String(remaining.length)) applyRollout();
      }}
    >
      <div class="field">
        <label for="confirm-input">Digite <span class="mono">{remaining.length}</span>, o número de equipamentos, para confirmar</label>
        <!-- svelte-ignore a11y_autofocus -->
        <input class="input" id="confirm-input" bind:value={confirmText} autocomplete="off" inputmode="numeric" autofocus />
      </div>
    </form>
    {#snippet footer()}
      <button class="btn" onclick={() => (confirming = false)}>Cancelar</button>
      <button class="btn danger" type="submit" form="confirm-form" disabled={confirmText.trim() !== String(remaining.length)}>Aplicar no lote</button>
    {/snippet}
  </Modal>
{/if}

{#if skipWarning}
  <Modal title="Pular o ensaio?" width={560} onclose={() => (skipWarning = false)}>
    <p class="confirm-text">
      Sem ensaio, o SelectAll não roda o SQL antes numa transação desfeita. Você não vê quantas linhas cada equipamento alteraria nem quem foge do padrão. O piloto e o lote gravam de verdade, e a auditoria registra que o ensaio foi pulado.
    </p>
    <p class="confirm-text">Use só com um SQL já testado. Os bloqueios de UPDATE e DELETE sem WHERE e de DDL continuam valendo.</p>
    {#snippet footer()}
      <button class="btn" onclick={() => (skipWarning = false)}>Manter o ensaio</button>
      <button class="btn danger" onclick={() => ((skipRehearsal = true), (skipWarning = false))}>Estou ciente, pular o ensaio</button>
    {/snippet}
  </Modal>
{/if}

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
  .store { display: flex; align-items: center; border-radius: var(--radius); }
  .store:hover, .store.open { background: var(--surface-sunken); }
  .store-pick { flex: 1; min-width: 0; display: grid; grid-template-columns: 20px 40px 1fr; align-items: center; gap: 6px; padding: 6px 4px 6px 8px; cursor: pointer; font-size: var(--text-sm); }
  .expand { flex: none; display: flex; align-items: center; gap: 4px; padding: 4px 6px; border: 0; background: none; color: var(--text-muted); cursor: pointer; border-radius: var(--radius); font: inherit; }
  .expand:hover:not(:disabled) { background: var(--surface); color: var(--text); }
  .expand:disabled { cursor: default; }
  .expand:disabled .chevron { visibility: hidden; }
  .chevron { width: 10px; font-size: var(--text-xs); }
  .store .count.partial { color: var(--brand); font-weight: 600; }
  .device-list { margin: 0 0 4px; padding: 2px 8px 6px 34px; border-left: 2px solid var(--border); margin-left: 17px; }
  .device { display: grid; grid-template-columns: 20px 1fr auto; align-items: center; gap: 6px; padding: 3px 0; font-size: var(--text-sm); cursor: pointer; }
  .device.off .name { color: var(--text-muted); }
  .device .name, .device .mono { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .skip { color: var(--err); }
  .device input { margin: 0; accent-color: var(--brand); }
  .store input { margin: 0; accent-color: var(--brand); }
  .store .code { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); }
  .store .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .store .count { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .targets footer { border-top: 1px solid var(--border); padding: 10px 16px; display: flex; justify-content: space-between; }

  .work { flex: 1; min-width: 0; display: flex; flex-direction: column; min-height: 0; }
  .editor-area { padding: 16px 20px 12px; border-bottom: 1px solid var(--border); background: var(--bg); flex: none; }
  .editor-head { display: flex; align-items: center; gap: 12px; margin-bottom: 10px; }
  .editor { border: 1px solid var(--border-strong); border-radius: var(--radius-lg); background: var(--surface); overflow: hidden; height: 150px; }
  .editor:focus-within { border-color: var(--brand); }
    .runbar { display: flex; align-items: center; gap: 12px; margin-top: 10px; font-size: var(--text-sm); flex-wrap: wrap; }
  .runbar strong { color: var(--text); font-weight: 500; }
  .btn.lg { padding: 8px 18px; font-size: var(--text-md); }

  .results { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .empty { margin: auto; text-align: center; color: var(--border-strong); padding: 40px; display: grid; justify-items: center; gap: 6px; }
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
  .grid.plain th { padding: 7px 12px; }
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

  .stepper { display: flex; padding: 14px 20px; border-bottom: 1px solid var(--border); flex: none; }
  .step { flex: 1; display: flex; gap: 10px; align-items: center; color: var(--text-muted); font-size: var(--text-sm); }
  .step .b { width: 24px; height: 24px; border-radius: 50%; border: 1.5px solid var(--border-strong); display: grid; place-items: center; font-size: 12px; font-weight: 600; }
  .step .hint { font-weight: 400; }
  .step.current { color: var(--text); font-weight: 500; }
  .step.current .b { border-color: var(--brand); color: var(--brand); }
  .step.done .b { background: var(--ok); border-color: var(--ok); color: var(--surface); }
  .step + .step::before { content: ""; width: 32px; height: 1px; background: var(--border-strong); margin-right: 10px; }
  .callout { margin: 12px 20px 0; padding: 12px 14px; border-radius: var(--radius-lg); font-size: var(--text-sm); display: flex; gap: 12px; align-items: center; flex: none; }
  .callout.info { background: var(--brand-soft); }
  .callout.attn { background: var(--warn-soft); }
  .callout.good { background: var(--ok-soft); }
  .callout.bad { background: var(--err-soft); }
  .callout strong { font-weight: 600; }
  .pilot { width: 280px; }
  .devices { margin-top: 12px; }
  tr.excluded td { color: var(--text-muted); }
  .confirm-text { margin: 0 0 10px; }
  .opened { max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .params { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 14px; margin-top: 10px; }
  .param { display: flex; align-items: center; gap: 6px; font-size: var(--text-sm); }
  .param .mono { color: var(--brand); font-weight: 600; }
  .param .input { width: 180px; }
  .param .input[aria-invalid="true"] { border-color: var(--warn); }
  .columns { padding: 0 20px 10px; align-items: center; }
  .compare { padding: 0 20px 16px; overflow: auto; flex: 1; display: grid; gap: 12px; align-content: start; }
  .group-card { padding: 14px 16px; display: grid; grid-template-columns: 1fr auto; gap: 6px 16px; }
  .group-card .vals { font-family: var(--font-mono); font-size: 13px; margin-top: 4px; }
  .group-card .who { grid-column: 1 / -1; font-size: var(--text-sm); color: var(--text-muted); }
  .group-card.minor { border-color: var(--warn); }
  .group-card summary { cursor: pointer; margin-top: 4px; width: fit-content; }
  .members { display: flex; flex-wrap: wrap; gap: 4px 16px; margin-top: 6px; color: var(--text); }
  .share { font-size: var(--text-xl); font-weight: 600; font-variant-numeric: tabular-nums; }
  .share .muted { font-size: var(--text-sm); font-weight: 400; }
  .confirm-sql { background: var(--surface-sunken); padding: 10px; border-radius: var(--radius); font: 12px/1.5 var(--font-mono); white-space: pre-wrap; margin: 0 0 12px; max-height: 200px; overflow: auto; }
</style>
