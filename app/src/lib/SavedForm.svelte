<script lang="ts">
  import { api, message, paramNames, type Profile, type SavedQuery, type SavedView } from "./api";
  import Modal from "./Modal.svelte";

  let {
    query,
    profiles,
    onsaved,
    onclose,
  }: {
    query: SavedQuery;
    profiles: Profile[];
    onsaved: (saved: SavedView) => void;
    onclose: () => void;
  } = $props();

  // Cópia de trabalho: a consulta original só muda quando o núcleo aceita a gravação.
  // svelte-ignore state_referenced_locally
  let form = $state<SavedQuery>({ ...query });
  let failure = $state("");
  let saving = $state(false);
  const names = $derived(paramNames(form.sql));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    saving = true;
    try {
      onsaved(await api.saveQuery($state.snapshot(form)));
    } catch (error) {
      failure = message(error);
    } finally {
      saving = false;
    }
  }
</script>

<Modal title={form.id ? "Editar consulta salva" : "Salvar consulta"} subtitle="Fica na biblioteca desta rede. Use {'{{nome}}'} no SQL para pedir um valor antes de executar." width={640} {onclose}>
  <form id="saved-form" onsubmit={submit}>
    <label class="field">
      <span class="label">Nome</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input" bind:value={form.name} maxlength="120" autofocus required />
    </label>
    <label class="field">
      <span class="label">Descrição <span class="hint">opcional</span></span>
      <input class="input" bind:value={form.description} maxlength="300" />
    </label>
    <label class="field">
      <span class="label">Perfil de sistema <span class="hint">opcional; define o dialeto da análise</span></span>
      <select class="input" value={form.profile_id ?? ""} onchange={(e) => (form.profile_id = e.currentTarget.value ? Number(e.currentTarget.value) : null)}>
        <option value="">Qualquer perfil</option>
        {#each profiles as profile (profile.id)}<option value={profile.id}>{profile.name}</option>{/each}
      </select>
    </label>
    <label class="field">
      <span class="label">SQL</span>
      <textarea class="input" bind:value={form.sql} spellcheck="false" required></textarea>
      <span class="hint">{names.length ? `Parâmetros: ${names.join(", ")}` : "Sem parâmetros"}</span>
    </label>
    {#if failure}<p class="error-text" role="alert">{failure}</p>{/if}
  </form>
  {#snippet footer()}
    <button class="btn" onclick={onclose}>Cancelar</button>
    <button class="btn primary" form="saved-form" disabled={saving || !form.name.trim() || !form.sql.trim()}>Salvar</button>
  {/snippet}
</Modal>
