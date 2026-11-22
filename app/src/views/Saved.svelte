<script lang="ts">
  import { api, fileStamp, message, plural, type Profile, type SavedQuery, type SavedView } from "../lib/api";
  import EmptyState from "../lib/EmptyState.svelte";
  import Modal from "../lib/Modal.svelte";
  import SavedForm from "../lib/SavedForm.svelte";

  let { chainId, onopen }: { chainId: number; onopen: (query: SavedView) => void } = $props();

  let queries = $state<SavedView[]>([]);
  let profiles = $state<Profile[]>([]);
  let loaded = $state(false);
  let failure = $state("");
  let notice = $state("");
  let editing = $state<SavedQuery | null>(null);
  let removing = $state<SavedView | null>(null);
  let fileInput = $state<HTMLInputElement>();

  const groups = $derived(group(queries));

  $effect(() => {
    load(chainId);
  });

  async function load(id: number) {
    try {
      const [list, fleet] = await Promise.all([api.savedQueries(id), api.fleet(id)]);
      queries = list;
      profiles = fleet.profiles;
      failure = "";
    } catch (error) {
      failure = message(error);
    }
    loaded = true;
  }

  function group(list: SavedView[]): [string, SavedView[]][] {
    const byProfile = new Map<string, SavedView[]>();
    for (const query of list) {
      const name = profiles.find((p) => p.id === query.profile_id)?.name ?? "";
      byProfile.set(name, [...(byProfile.get(name) ?? []), query]);
    }
    return [...byProfile].sort(([a], [b]) => (a === "" ? 1 : b === "" ? -1 : a.localeCompare(b, "pt-BR")));
  }

  /** Partes do SQL com os parâmetros separados, para destacar sem montar HTML. */
  function parts(sql: string): { text: string; param: boolean }[] {
    return sql.split(/(\{\{[A-Za-z_][A-Za-z0-9_]*\}\})/).map((text) => ({ text, param: /^\{\{.*\}\}$/.test(text) }));
  }

  function create() {
    editing = { id: 0, chain_id: chainId, name: "", description: "", profile_id: null, sql: "" };
  }

  function saved(view: SavedView) {
    notice = `Consulta "${view.name}" salva.`;
    editing = null;
    load(chainId);
  }

  async function remove() {
    if (!removing) return;
    try {
      await api.deleteQuery(removing.id);
      notice = `Consulta "${removing.name}" excluída.`;
      removing = null;
      await load(chainId);
    } catch (error) {
      failure = message(error);
    }
  }

  async function exportLibrary() {
    try {
      notice = `Biblioteca salva em ${await api.exportLibrary(chainId, `selectall-consultas-${fileStamp()}`)}`;
      failure = "";
    } catch (error) {
      failure = message(error);
    }
  }

  async function importLibrary(event: Event & { currentTarget: HTMLInputElement }) {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = "";
    if (!file) return;
    try {
      const { added, replaced } = await api.importLibrary(chainId, await file.text());
      const pieces = [plural(added, "consulta nova", "consultas novas")];
      if (replaced) pieces.push(`${plural(replaced, "substituída", "substituídas")} por ter o mesmo nome`);
      notice = `${file.name}: ${pieces.join(", ")}.`;
      failure = "";
      await load(chainId);
    } catch (error) {
      failure = `${file.name}: ${message(error)}`;
    }
  }
</script>

<section class="saved">
  <div class="head">
    <span class="muted">Compartilhe exportando a biblioteca para a equipe. Ao importar, consulta com o mesmo nome é substituída.</span>
    <span class="grow"></span>
    <input bind:this={fileInput} type="file" accept=".json,application/json" hidden onchange={importLibrary} />
    <button class="btn" onclick={() => fileInput?.click()}>Importar</button>
    <button class="btn" onclick={exportLibrary} disabled={!queries.length}>Exportar</button>
    <button class="btn primary" onclick={create}>Nova consulta</button>
  </div>
  {#if failure}<p class="error-text pad" role="alert">{failure}</p>{/if}
  {#if notice}<p class="notice selectable" role="status">{notice}</p>{/if}
  {#if loaded && !queries.length && !failure}
    <EmptyState title="Nenhuma consulta salva">
      <p>Salve um SQL pela Consulta ou crie aqui. Parâmetros como {"{{codigo_produto}}"} são pedidos antes de executar.</p>
    </EmptyState>
  {:else}
    <div class="list">
      {#each groups as [profile, list] (profile)}
        <div class="section-title group-label">{profile ? `Perfil ${profile}` : "Qualquer perfil"}</div>
        <div class="q-grid">
          {#each list as query (query.id)}
            <article class="card q-card">
              <div class="row">
                <h3 class="grow">{query.name}</h3>
                <span class="pill {query.write ? 'warn' : 'run'}">{query.write ? "Escrita" : "Leitura"}</span>
              </div>
              {#if query.description}<p>{query.description}</p>{/if}
              <pre class="selectable">{#each parts(query.sql) as part, index (index)}{#if part.param}<span class="param">{part.text}</span>{:else}{part.text}{/if}{/each}</pre>
              <span class="muted small">{query.params.length ? `Parâmetros: ${query.params.join(", ")}` : "Sem parâmetros"}</span>
              <div class="row">
                <span class="grow"></span>
                <button class="btn ghost small danger" onclick={() => (removing = query)}>Excluir</button>
                <button class="btn ghost small" onclick={() => (editing = { ...query })}>Editar</button>
                <button class="btn small" onclick={() => onopen(query)}>Abrir em {query.write ? "Corrigir" : "Consultar"}</button>
              </div>
            </article>
          {/each}
        </div>
      {/each}
    </div>
  {/if}
</section>

{#if editing}
  <SavedForm query={editing} {profiles} onsaved={saved} onclose={() => (editing = null)} />
{/if}

{#if removing}
  <Modal title="Excluir {removing.name}?" width={460} onclose={() => (removing = null)}>
    <p class="confirm">A consulta sai da biblioteca desta rede. Execuções no Histórico continuam.</p>
    {#snippet footer()}
      <button class="btn" onclick={() => (removing = null)}>Cancelar</button>
      <button class="btn primary danger-fill" onclick={remove}>Excluir</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .saved { flex: 1; display: flex; flex-direction: column; min-height: 0; }
  .head { display: flex; align-items: center; gap: 10px; padding: 14px 20px; font-size: var(--text-sm); flex: none; }
  .grow { flex: 1; }
  .pad { padding: 0 20px 10px; }
  .notice { margin: 0 20px 8px; padding: 6px 10px; background: var(--surface-sunken); border-radius: var(--radius); font-size: var(--text-sm); word-break: break-all; }
  .list { flex: 1; overflow: auto; padding: 0 20px 24px; }
  .group-label { margin: 18px 0 10px; }
  .group-label:first-child { margin-top: 4px; }
  .q-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 12px; }
  .q-card { padding: 14px 16px; display: flex; flex-direction: column; gap: 8px; }
  .q-card h3 { margin: 0; font-size: var(--text-md); font-weight: 600; }
  .q-card p { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .q-card pre { margin: 0; font: 12px/1.5 var(--font-mono); background: var(--surface-sunken); border-radius: var(--radius); padding: 8px 10px; overflow: hidden; white-space: pre-wrap; max-height: 90px; }
  .q-card .row { display: flex; align-items: center; gap: 8px; font-size: var(--text-xs); }
  .param { color: var(--brand); font-weight: 600; }
  .small { font-size: var(--text-xs); }
  .confirm { margin: 0; font-size: var(--text-sm); }
  .danger-fill { background: var(--err); border-color: var(--err); }
</style>
