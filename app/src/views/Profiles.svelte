<script lang="ts">
  import { untrack } from "svelte";
  import { api, defaultPort, engineLabel, message, type Device, type Engine, type Profile, type ProfileView } from "../lib/api";
  import Modal from "../lib/Modal.svelte";

  let {
    chainId,
    profiles,
    devices,
    onchange,
    onclose,
  }: {
    chainId: number;
    profiles: ProfileView[];
    devices: Device[];
    onchange: () => Promise<void>;
    onclose: () => void;
  } = $props();

  const blank = (): Profile => ({ id: 0, chain_id: chainId, name: "", engine: "mysql", port: 3306, database: "", username: "", tls: false });

  // Sem perfil nenhum, a única coisa a fazer aqui é criar o primeiro.
  const startEmpty = untrack(() => profiles.length === 0);
  let editing = $state<Profile | null>(startEmpty ? blank() : null);
  let hadPassword = $state(false);
  let password = $state("");
  let changePassword = $state(startEmpty);
  let failure = $state("");
  let saving = $state(false);
  let confirmDelete = $state(false);

  function open(profile: ProfileView | null) {
    editing = profile ? { ...profile } : blank();
    hadPassword = profile?.has_password ?? false;
    changePassword = !profile;
    password = "";
    failure = "";
    confirmDelete = false;
  }

  const engineNote: Record<Engine, string> = {
    mysql: "",
    postgres: "Sem banco, o PostgreSQL abre o banco com o nome do usuário.",
    sqlserver: "Sem banco, o SQL Server abre o banco padrão do login. Sem TLS, só o login vai cifrado e o certificado do servidor não é conferido.",
  };

  function engineChanged(engine: Engine) {
    if (!editing) return;
    if (editing.port === defaultPort[editing.engine]) editing.port = defaultPort[engine];
    editing.engine = engine;
  }

  function usage(id: number): string {
    const count = devices.filter((d) => d.profile_ids.includes(id)).length;
    return count === 0 ? "Sem equipamentos" : `${count.toLocaleString("pt-BR")} equipamento${count === 1 ? "" : "s"}`;
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!editing) return;
    saving = true;
    failure = "";
    try {
      await api.saveProfile($state.snapshot(editing), changePassword ? password : null);
      password = "";
      await onchange();
      editing = null;
    } catch (error) {
      failure = message(error);
    }
    saving = false;
  }

  async function remove() {
    if (!editing) return;
    saving = true;
    try {
      await api.deleteProfile(editing.id);
      await onchange();
      editing = null;
    } catch (error) {
      failure = message(error);
    }
    saving = false;
  }
</script>

<Modal title="Perfis de sistema" subtitle="Como falar com cada sistema de PDV. Senhas ficam no cofre do sistema operacional." {onclose}>
  {#if editing}
    <form id="profile-form" onsubmit={save}>
      <div class="grid">
        <div class="field wide">
          <label for="p-name">Nome</label>
          <!-- svelte-ignore a11y_autofocus -->
          <input id="p-name" class="input" bind:value={editing.name} maxlength="60" autofocus />
        </div>
        <div class="field">
          <label for="p-engine">Motor</label>
          <select id="p-engine" class="input" value={editing.engine} onchange={(e) => engineChanged(e.currentTarget.value as Engine)}>
            {#each Object.entries(engineLabel) as [value, label] (value)}<option {value}>{label}</option>{/each}
          </select>
        </div>
        <div class="field">
          <label for="p-port">Porta</label>
          <input id="p-port" class="input mono" type="number" min="1" max="65535" bind:value={editing.port} />
        </div>
        <div class="field">
          <label for="p-db">Banco <span class="hint">opcional</span></label>
          <input id="p-db" class="input mono" bind:value={editing.database} placeholder="pdv" />
        </div>
        <div class="field">
          <label for="p-user">Usuário</label>
          <input id="p-user" class="input mono" bind:value={editing.username} placeholder="leitura" autocomplete="off" aria-describedby="p-user-hint" />
          <span class="hint" id="p-user-hint">Para consultar, um usuário só com SELECT basta. Escrita só é preciso para Corrigir.</span>
        </div>
        <div class="field wide">
          <span class="label">Senha</span>
          {#if changePassword}
            <input class="input mono" type="password" bind:value={password} aria-label="Senha" autocomplete="new-password" placeholder={hadPassword ? "Nova senha" : ""} />
          {:else}
            <div class="row">
              <span class="pill {hadPassword ? 'ok' : 'warn'}">{hadPassword ? "Guardada no cofre" : "Sem senha no cofre"}</span>
              <button type="button" class="btn small" onclick={() => (changePassword = true)}>{hadPassword ? "Trocar senha" : "Definir senha"}</button>
            </div>
          {/if}
        </div>
        <label class="check wide"><input type="checkbox" bind:checked={editing.tls} />Exigir conexão criptografada (TLS)</label>
      </div>
      {#if engineNote[editing.engine]}<p class="muted note">{engineNote[editing.engine]}</p>{/if}
      {#if failure}<p class="error-text" role="alert">{failure}</p>{/if}
    </form>
  {:else}
    <div class="profiles">
      {#each profiles as profile (profile.id)}
        <button class="card profile" onclick={() => open(profile)}>
          <strong>{profile.name}</strong>
          <span class="muted use">{usage(profile.id)}</span>
          <span class="meta">{engineLabel[profile.engine]} · porta {profile.port}{profile.database ? ` · banco ${profile.database}` : ""} · usuário {profile.username || "–"}{profile.has_password ? "" : " · sem senha"}</span>
        </button>
      {:else}
        <p class="muted empty">Nenhum perfil ainda. Crie um para dizer como o app entra no banco dos PDVs.</p>
      {/each}
    </div>
    {#if failure}<p class="error-text" role="alert">{failure}</p>{/if}
  {/if}

  {#snippet footer()}
    {#if editing}
      {#if editing.id !== 0}
        {#if confirmDelete}
          <span class="muted confirm">Excluir {editing.name} e a senha guardada?</span>
          <button class="btn danger" disabled={saving} onclick={remove}>Excluir</button>
        {:else}
          <button class="btn ghost danger" onclick={() => (confirmDelete = true)}>Excluir perfil</button>
        {/if}
      {/if}
      <span class="grow"></span>
      <button class="btn" onclick={() => (profiles.length ? (editing = null) : onclose())}>{profiles.length ? "Voltar" : "Cancelar"}</button>
      <button class="btn primary" form="profile-form" disabled={saving || !editing.name.trim()}>Salvar perfil</button>
    {:else}
      <button class="btn" onclick={() => open(null)}>Novo perfil</button>
      <span class="grow"></span>
      <button class="btn primary" onclick={onclose}>Fechar</button>
    {/if}
  {/snippet}
</Modal>

<style>
  .profiles { display: grid; gap: 8px; }
  .profile { padding: 10px 12px; display: grid; grid-template-columns: 1fr auto; gap: 2px 10px; font-size: var(--text-sm); text-align: left; cursor: pointer; }
  .profile:hover { border-color: var(--border-strong); }
  .use { font-size: var(--text-xs); }
  .meta { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); grid-column: 1 / -1; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0 12px; align-items: start; }
  .wide { grid-column: 1 / -1; }
  .row { display: flex; align-items: center; gap: 10px; }
  .note { font-size: var(--text-sm); margin: 4px 0 0; }
  .empty { margin: 8px 0; }
  .grow { flex: 1; }
  .confirm { font-size: var(--text-sm); }
  .check { margin-bottom: 4px; }
</style>
