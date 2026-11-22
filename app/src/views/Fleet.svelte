<script lang="ts">
  import { api, message, plural, type Device, type Fleet, type ScanHit, type Store } from "../lib/api";
  import Asterisk from "../lib/Asterisk.svelte";
  import AddStores from "./AddStores.svelte";
  import Profiles from "./Profiles.svelte";

  let { chainId }: { chainId: number } = $props();

  let fleet = $state<Fleet>({ stores: [], devices: [], profiles: [], rules: [] });
  let loaded = $state(false);
  let selectedId = $state<number | null>(null);
  let filter = $state("");
  let modal = $state<"add" | "profiles" | null>(null);
  let failure = $state("");
  let toast = $state("");
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  /** Último resultado de varredura por endereço, só nesta sessão. */
  let online = $state<Record<string, boolean>>({});
  let scanning = $state(false);

  let storeForm = $state<Store | null>(null);
  let deviceForm = $state<Device | null>(null);
  let confirming = $state<"store" | "device" | null>(null);
  let saving = $state(false);

  const byStore = $derived(groupByStore(fleet.devices));
  const kinds = $derived(topKinds());
  const selected = $derived(fleet.stores.find((s) => s.id === selectedId) ?? null);
  const selectedDevices = $derived(selected ? (byStore.get(selected.id) ?? []) : []);
  /** Traz o formulário do equipamento para a vista, põe o cursor no primeiro campo e fecha com Esc. */
  function reveal(form: HTMLFormElement) {
    form.scrollIntoView({ block: "nearest", behavior: "smooth" });
    form.querySelector<HTMLInputElement>("input")?.focus({ preventScroll: true });
    const close = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      deviceForm = null;
      confirming = null;
    };
    form.addEventListener("keydown", close);
    return { destroy: () => form.removeEventListener("keydown", close) };
  }

  const available = $derived(fleet.profiles.filter((p) => !deviceForm?.profile_ids.includes(p.id)));
  const profileName = $derived(new Map(fleet.profiles.map((p) => [p.id, p.name])));
  const visible = $derived(
    fleet.stores.filter((s) => {
      const term = filter.trim().toLowerCase();
      return !term || s.code.toLowerCase().includes(term) || s.name.toLowerCase().includes(term);
    }),
  );

  $effect(() => {
    load(chainId);
  });

  async function load(id: number) {
    try {
      fleet = await api.fleet(id);
      failure = "";
      if (!fleet.stores.some((s) => s.id === selectedId)) selectedId = fleet.stores[0]?.id ?? null;
    } catch (error) {
      failure = message(error);
    }
    loaded = true;
  }

  const reload = () => load(chainId);

  function groupByStore(devices: Device[]): Map<number, Device[]> {
    const groups = new Map<number, Device[]>();
    for (const device of devices) {
      const group = groups.get(device.store_id);
      if (group) group.push(device);
      else groups.set(device.store_id, [device]);
    }
    return groups;
  }

  /** Tipos da regra primeiro, depois os mais frequentes; no máximo quatro colunas. */
  function topKinds(): string[] {
    const counts = new Map<string, number>();
    for (const rule of fleet.rules) counts.set(rule.kind, Number.MAX_SAFE_INTEGER - counts.size);
    for (const device of fleet.devices) if (!fleet.rules.some((r) => r.kind === device.kind)) counts.set(device.kind, (counts.get(device.kind) ?? 0) + 1);
    return [...counts].sort((a, b) => b[1] - a[1]).slice(0, 4).map(([kind]) => kind);
  }

  function count(storeId: number, kind: string): number {
    return (byStore.get(storeId) ?? []).filter((d) => d.kind === kind).length;
  }

  function availability(storeId: number): { up: number; total: number } | null {
    const devices = byStore.get(storeId) ?? [];
    if (!devices.length || !devices.some((d) => d.host in online)) return null;
    return { up: devices.filter((d) => online[d.host]).length, total: devices.length };
  }

  function show(text: string) {
    toast = text;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = ""), 3200);
  }

  function choose(store: Store) {
    selectedId = store.id;
    storeForm = null;
    deviceForm = null;
    confirming = null;
  }

  async function scanSelected() {
    if (!selected) return;
    scanning = true;
    try {
      const hits: ScanHit[] = await api.scanStore(chainId, selected.id);
      for (const hit of hits) online[hit.host] = hit.open;
      const up = hits.filter((h) => h.open).length;
      show(`Varredura: ${up} de ${hits.length} responderam`);
    } catch (error) {
      failure = message(error);
    }
    scanning = false;
  }

  async function saveStore(event: SubmitEvent) {
    event.preventDefault();
    if (!storeForm) return;
    saving = true;
    try {
      const saved = await api.saveStore($state.snapshot(storeForm));
      await reload();
      selectedId = saved.id;
      storeForm = null;
      failure = "";
    } catch (error) {
      failure = message(error);
    }
    saving = false;
  }

  async function saveDevice(event: SubmitEvent) {
    event.preventDefault();
    if (!deviceForm) return;
    saving = true;
    try {
      await api.saveDevice($state.snapshot(deviceForm));
      await reload();
      deviceForm = null;
      failure = "";
    } catch (error) {
      failure = message(error);
    }
    saving = false;
  }

  async function removeSelected() {
    saving = true;
    try {
      if (confirming === "store" && selected) {
        await api.deleteStore(selected.id);
        show(`Loja ${selected.code} excluída`);
        selectedId = null;
      } else if (confirming === "device" && deviceForm) {
        await api.deleteDevice(deviceForm.id);
        show(`${deviceForm.name} excluído`);
        deviceForm = null;
      }
      confirming = null;
      await reload();
    } catch (error) {
      failure = message(error);
    }
    saving = false;
  }

  function newDevice() {
    if (!selected) return;
    const profiles = selectedDevices[0]?.profile_ids ?? (fleet.profiles[0] ? [fleet.profiles[0].id] : []);
    deviceForm = { id: 0, store_id: selected.id, name: "", kind: kinds[0] ?? "PDV", host: "", profile_ids: [...profiles] };
    confirming = null;
  }

  function profileSummary(devices: Device[]): string {
    const orders = new Map<string, number>();
    for (const device of devices) {
      const key = device.profile_ids.map((id) => profileName.get(id) ?? "?").join(", depois ");
      orders.set(key, (orders.get(key) ?? 0) + 1);
    }
    const [top] = [...orders].sort((a, b) => b[1] - a[1]);
    if (!top) return "–";
    return top[0] || "Sem perfil";
  }

  async function added(summary: string) {
    modal = null;
    await reload();
    show(summary);
  }
</script>

{#if !loaded}
  <div class="fill"></div>
{:else if fleet.stores.length === 0}
  <section class="empty">
    <Asterisk size={40} />
    <h2>Nenhuma loja ainda</h2>
    <p class="muted">Cadastre um perfil de sistema e adicione as lojas pela regra de endereçamento, colando uma lista, por planilha ou varrendo uma faixa.</p>
    {#if failure}<p class="error-text" role="alert">{failure}</p>{/if}
    <div class="row">
      <button class="btn" onclick={() => (modal = "profiles")}>Perfis de sistema{fleet.profiles.length ? ` · ${fleet.profiles.length}` : ""}</button>
      <button class="btn primary" onclick={() => (modal = "add")}>Adicionar lojas</button>
    </div>
  </section>
{:else}
  <div class="fleet">
    <div class="pane">
      <div class="pane-head">
        <h2>Lojas</h2>
        <span class="muted">{plural(fleet.stores.length, "loja", "lojas")} · {plural(fleet.devices.length, "equipamento", "equipamentos")}</span>
        <span class="grow"></span>
        <input class="input search" bind:value={filter} placeholder="Filtrar por código ou nome" aria-label="Filtrar lojas" />
        <button class="btn" onclick={() => (modal = "profiles")}>Perfis de sistema</button>
        <button class="btn primary" onclick={() => (modal = "add")}>Adicionar lojas</button>
      </div>
      {#if failure}<p class="error-text banner" role="alert">{failure}</p>{/if}
      <div class="table-wrap stores">
        <table>
          <thead>
            <tr>
              <th>Código</th><th>Loja</th><th>Sub-rede</th>
              {#each kinds as kind (kind)}<th class="num">{kind}</th>{/each}
              <th>Disponíveis agora</th>
            </tr>
          </thead>
          <tbody>
            {#each visible as store (store.id)}
              {@const status = availability(store.id)}
              <tr class:selected={store.id === selectedId} onclick={() => choose(store)}>
                <td class="mono">{store.code}</td>
                <td>{store.name || "–"}</td>
                <td class="data muted">{store.subnet || "–"}</td>
                {#each kinds as kind (kind)}<td class="num">{count(store.id, kind) || "–"}</td>{/each}
                <td>
                  {#if status}<span class="pill {status.up === status.total ? 'ok' : 'off'}">{status.up}/{status.total}</span>
                  {:else}<span class="muted">não varrida</span>{/if}
                </td>
              </tr>
            {:else}
              <tr><td colspan={4 + kinds.length} class="muted">Nenhuma loja com “{filter}”.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>

    <aside class="detail">
      {#if selected}
        <section>
          {#if storeForm}
            <form onsubmit={saveStore} class="form">
              <div class="field"><label for="s-code">Código</label><input id="s-code" class="input mono" bind:value={storeForm.code} /></div>
              <div class="field"><label for="s-name">Nome</label><input id="s-name" class="input" bind:value={storeForm.name} /></div>
              <div class="field"><label for="s-sub">Sub-rede <span class="hint">valor de {"{sub}"} na regra</span></label><input id="s-sub" class="input mono" bind:value={storeForm.subnet} /></div>
              <div class="row">
                <button class="btn primary" disabled={saving || !storeForm.code.trim()}>Salvar</button>
                <button type="button" class="btn" onclick={() => (storeForm = null)}>Cancelar</button>
              </div>
            </form>
          {:else}
            <div class="section-title">Loja {selected.code}</div>
            <h2 class="store-name">{selected.name || "Sem nome"}</h2>
            <dl class="kv">
              <dt>Sub-rede</dt><dd class="mono">{selected.subnet || "–"}</dd>
              <dt>Perfis</dt><dd>{profileSummary(selectedDevices)}</dd>
              <dt>Equipamentos</dt><dd>{selectedDevices.length}</dd>
            </dl>
            <div class="row actions">
              <button class="btn" onclick={() => ((storeForm = { ...selected }), (confirming = null))}>Editar</button>
              <button class="btn" disabled={scanning || !selectedDevices.length} onclick={scanSelected}>{scanning ? "Varrendo…" : "Varrer agora"}</button>
              <span class="grow"></span>
              {#if confirming === "store"}
                <button class="btn danger" disabled={saving} onclick={removeSelected}>Excluir loja e {selectedDevices.length} equip.</button>
              {:else}
                <button class="btn ghost danger" onclick={() => (confirming = "store")}>Excluir</button>
              {/if}
            </div>
          {/if}
        </section>

        <section>
          <div class="row head">
            <span class="section-title">Equipamentos · {selectedDevices.length}</span>
            <span class="grow"></span>
            <button class="btn small" onclick={newDevice}>Novo equipamento</button>
          </div>
          {#if deviceForm?.id === 0}{@render deviceEditor()}{/if}
          {#each selectedDevices as device (device.id)}
            {#if deviceForm?.id === device.id}
              {@render deviceEditor()}
            {:else}
            <button class="equip" onclick={() => ((deviceForm = { ...device, profile_ids: [...device.profile_ids] }), (confirming = null))}>
              <span>{device.name}<span class="muted kind"> · {device.kind}</span></span>
              <span class="mono muted ip">{device.host}</span>
              <span>
                {#if device.host in online}
                  <span class="pill {online[device.host] ? 'ok' : 'off'}">{online[device.host] ? "Online" : "Offline"}</span>
                {/if}
              </span>
            </button>
            {/if}
          {:else}
            <p class="muted">Nenhum equipamento nesta loja.</p>
          {/each}
        </section>
      {:else}
        <section><p class="muted">Selecione uma loja.</p></section>
      {/if}
    </aside>
  </div>
{/if}

{#snippet deviceEditor()}
  {#if deviceForm}
    <form onsubmit={saveDevice} class="form device-form" use:reveal>
      <div class="two">
        <div class="field"><label for="d-name">Nome</label><input id="d-name" class="input" bind:value={deviceForm.name} placeholder="PDV 01" /></div>
        <div class="field"><label for="d-kind">Tipo</label><input id="d-kind" class="input" bind:value={deviceForm.kind} /></div>
      </div>
      <div class="field"><label for="d-host">Endereço</label><input id="d-host" class="input mono" bind:value={deviceForm.host} placeholder="10.101.3.1" /></div>
      <div class="field">
        <span class="label">Perfis, na ordem de tentativa</span>
        {#each deviceForm.profile_ids as id, index (id)}
          <div class="row chip-row">
            <span class="mono muted">{index + 1}.</span><span class="grow">{profileName.get(id) ?? "Perfil removido"}</span>
            <button type="button" class="btn ghost small" onclick={() => deviceForm?.profile_ids.splice(index, 1)}>Remover</button>
          </div>
        {/each}
        {#if available.length}
          <select class="input" aria-label="Adicionar perfil" value="" onchange={(e) => { deviceForm?.profile_ids.push(Number(e.currentTarget.value)); e.currentTarget.value = ""; }}>
            <option value="" disabled>Adicionar perfil…</option>
            {#each available as profile (profile.id)}<option value={profile.id}>{profile.name}</option>{/each}
          </select>
        {/if}
      </div>
      <div class="row">
        <button class="btn primary" disabled={saving || !deviceForm.host.trim() || !deviceForm.kind.trim()}>Salvar</button>
        <button type="button" class="btn" onclick={() => ((deviceForm = null), (confirming = null))}>Cancelar</button>
        <span class="grow"></span>
        {#if deviceForm?.id !== 0}
          {#if confirming === "device"}
            <button type="button" class="btn danger" disabled={saving} onclick={removeSelected}>Confirmar exclusão</button>
          {:else}
            <button type="button" class="btn ghost danger" onclick={() => (confirming = "device")}>Excluir</button>
          {/if}
        {/if}
      </div>
    </form>
  {/if}
{/snippet}

{#if modal === "add"}
  <AddStores {chainId} {fleet} onadded={added} onclose={() => (modal = null)} />
{:else if modal === "profiles"}
  <Profiles {chainId} profiles={fleet.profiles} devices={fleet.devices} onchange={reload} onclose={() => (modal = null)} />
{/if}

{#if toast}<div class="toast" role="status">{toast}</div>{/if}

<style>
  .fill { flex: 1; }
  .empty { flex: 1; display: grid; place-content: center; justify-items: center; gap: 6px; text-align: center; color: var(--border-strong); padding: 40px; }
  .empty h2 { color: var(--text); font-size: var(--text-lg); font-weight: 600; margin: 4px 0 0; }
  .empty p { margin: 0 0 8px; max-width: 52ch; }
  .fleet { flex: 1; display: grid; grid-template-columns: minmax(520px, 1fr) 400px; min-height: 0; }
  .pane { display: flex; flex-direction: column; min-height: 0; }
  .pane-head { display: flex; align-items: center; gap: 10px; padding: 16px 20px 10px; flex: none; }
  .pane-head h2 { font-size: var(--text-lg); font-weight: 600; margin: 0; }
  .search { width: 220px; }
  .banner { padding: 0 20px 8px; }
  .stores { flex: 1; min-height: 0; margin: 4px 20px 16px; }
  .stores tr { cursor: pointer; }
  .detail { border-left: 1px solid var(--border); background: var(--surface); overflow: auto; }
  .detail section { padding: 16px 20px; border-bottom: 1px solid var(--border); }
  .store-name { margin: 4px 0 0; font-size: var(--text-xl); font-weight: 600; }
  .kv { display: grid; grid-template-columns: 110px 1fr; gap: 6px 12px; font-size: var(--text-sm); margin: 10px 0 0; }
  .kv dt { color: var(--text-muted); }
  .kv dd { margin: 0; }
  .row { display: flex; align-items: center; gap: 8px; }
  .actions { margin-top: 14px; }
  .head { margin-bottom: 6px; }
  .grow { flex: 1; }
  .form { margin-top: 4px; }
  .device-form { padding: 12px; margin: 6px 0 10px; background: var(--surface-sunken); border-radius: var(--radius); }
  .two { display: grid; grid-template-columns: 1fr 1fr; gap: 0 10px; }
  .chip-row { font-size: var(--text-sm); }
  .equip { display: grid; grid-template-columns: 1fr 110px 70px; gap: 8px; align-items: center; font-size: var(--text-sm); padding: 5px 4px; border: 0; border-bottom: 1px solid var(--border); background: transparent; width: 100%; text-align: left; cursor: pointer; }
  .equip:hover { background: var(--surface-sunken); }
  .kind, .ip { font-size: var(--text-xs); }
  .toast { position: fixed; bottom: 20px; left: 50%; transform: translateX(-50%); background: var(--text); color: var(--bg); padding: 10px 16px; border-radius: var(--radius); font-size: var(--text-sm); z-index: 50; }
</style>
